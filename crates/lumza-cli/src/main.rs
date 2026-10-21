//! `lumza` — stage 1 CLI prototype: list / get / set / caps.

use anyhow::{anyhow, bail, Result};
use clap::{Parser, Subcommand};
use lumza_core::backend::verified_capabilities;
use lumza_core::mock::MockBackend;
use lumza_core::scale::percent_to_raw;
use lumza_core::{DisplayBackend, Feature, MonitorInfo};

#[derive(Parser)]
#[command(name = "lumzactl", version, about = "Control monitors over DDC/CI")]
struct Cli {
    /// Use the in-memory mock backend instead of real hardware
    #[arg(long, global = true)]
    mock: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List monitors with their stable IDs and supported features
    List,
    /// Read a feature: brightness | contrast | volume | 0xNN
    Get { monitor: String, feature: String },
    /// Write a feature. VALUE is a percentage (0-100); use --raw for the raw VCP value
    Set {
        monitor: String,
        feature: String,
        value: u32,
        #[arg(long)]
        raw: bool,
    },
    /// Print the capabilities string, parsed VCP codes and verification result
    Caps { monitor: String },
}

fn backend(mock: bool) -> Box<dyn DisplayBackend> {
    if mock {
        return Box::new(MockBackend::demo());
    }
    #[cfg(windows)]
    {
        Box::new(lumza_core::dxva2::Dxva2Backend::new())
    }
    #[cfg(not(windows))]
    {
        eprintln!("real backend is Windows-only, falling back to --mock");
        Box::new(MockBackend::demo())
    }
}

/// Monitor selector: 1-based number as printed by `list`, an exact ID, or a name substring.
fn pick<'a>(mons: &'a [MonitorInfo], sel: &str) -> Result<&'a MonitorInfo> {
    if let Ok(n) = sel.parse::<usize>() {
        if let Some(m) = mons.get(n.wrapping_sub(1)) {
            return Ok(m);
        }
    }
    let low = sel.to_lowercase();
    mons.iter()
        .find(|m| m.id.eq_ignore_ascii_case(sel))
        .or_else(|| mons.iter().find(|m| m.name.to_lowercase().contains(&low)))
        .ok_or_else(|| anyhow!("no monitor matches '{sel}' (run `lumza list`)"))
}

fn parse_code(s: &str) -> Result<u8> {
    if let Ok(f) = s.parse::<Feature>() {
        return Ok(f.vcp());
    }
    let hex = s.trim_start_matches("0x").trim_start_matches("0X");
    u8::from_str_radix(hex, 16).map_err(|_| anyhow!("unknown feature '{s}' (use brightness|contrast|volume|0xNN)"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut be = backend(cli.mock);
    let mons = be.list()?;
    if mons.is_empty() {
        bail!("no DDC/CI monitors found");
    }

    match cli.cmd {
        Cmd::List => {
            for (i, m) in mons.iter().enumerate() {
                println!(
                    "{}. {}  [{}]  id={}  ({})",
                    i + 1,
                    m.name,
                    m.device,
                    m.id,
                    if m.connection == lumza_core::types::Connection::Ddc { "DDC/CI" } else { "internal" }
                );
                for f in Feature::ALL {
                    match be.get_vcp(&m.id, f.vcp()) {
                        Ok(v) => println!(
                            "     {:<10} 0x{:02X}  {:>3}%  (raw {}/{})",
                            f.name(),
                            f.vcp(),
                            v.percent(),
                            v.current,
                            v.max
                        ),
                        Err(_) => println!("     {:<10} 0x{:02X}  not supported", f.name(), f.vcp()),
                    }
                }
            }
        }
        Cmd::Get { monitor, feature } => {
            let m = pick(&mons, &monitor)?;
            let code = parse_code(&feature)?;
            let v = be.get_vcp(&m.id, code)?;
            println!("{}: 0x{:02X} = {} / {}  ({}%)", m.name, code, v.current, v.max, v.percent());
        }
        Cmd::Set { monitor, feature, value, raw } => {
            let m = pick(&mons, &monitor)?;
            let code = parse_code(&feature)?;
            let target = if raw {
                value
            } else {
                if value > 100 {
                    bail!("percentage must be 0-100 (use --raw for raw values)");
                }
                let max = be.get_vcp(&m.id, code)?.max;
                percent_to_raw(value as u8, max)
            };
            be.set_vcp(&m.id, code, target)?;
            let v = be.get_vcp(&m.id, code)?;
            println!("{}: 0x{:02X} set -> now {} / {} ({}%)", m.name, code, v.current, v.max, v.percent());
        }
        Cmd::Caps { monitor } => {
            let m = pick(&mons, &monitor)?;
            let raw = be.capabilities_string(&m.id)?;
            println!("raw: {raw}");
            let (caps, dropped) = verified_capabilities(be.as_mut(), &m.id)?;
            println!(
                "model: {}  mccs: {}",
                caps.model.as_deref().unwrap_or("?"),
                caps.mccs_version.as_deref().unwrap_or("?")
            );
            let codes: Vec<String> = caps.vcp.keys().map(|c| format!("{c:02X}")).collect();
            println!("vcp (verified): {}", codes.join(" "));
            if !dropped.is_empty() {
                let d: Vec<String> = dropped.iter().map(|c| format!("{c:02X}")).collect();
                println!("claimed but unresponsive (hidden): {}", d.join(" "));
            }
            for f in Feature::ALL {
                println!("  {:<10} 0x{:02X}: {}", f.name(), f.vcp(), if caps.supports(f.vcp()) { "yes" } else { "no" });
            }
        }
    }
    Ok(())
}

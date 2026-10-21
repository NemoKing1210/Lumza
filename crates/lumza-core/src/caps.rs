//! Parser for DDC/CI capabilities strings, e.g.
//! `(prot(monitor)type(lcd)model(XYZ)cmds(01 02)vcp(02 10 12 14(01 05) 60(0F 11) 62)mccs_ver(2.2))`.

use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub model: Option<String>,
    pub mccs_version: Option<String>,
    /// VCP code -> allowed discrete values (empty for continuous features).
    pub vcp: BTreeMap<u8, Vec<u8>>,
}

impl Capabilities {
    pub fn supports(&self, code: u8) -> bool {
        self.vcp.contains_key(&code)
    }
}

pub fn parse(s: &str) -> Capabilities {
    let mut caps = Capabilities::default();
    let chars: Vec<char> = s.chars().collect();
    // Groups are `name(body)`, usually wrapped in one outer pair of parens.
    let mut i = usize::from(chars.first() == Some(&'('));
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == ')' {
            break;
        }
        let start = i;
        while i < chars.len() && chars[i] != '(' && chars[i] != ')' {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        let name = name.trim().to_ascii_lowercase();
        if i >= chars.len() || chars[i] != '(' {
            continue;
        }
        let (body, next) = balanced(&chars, i);
        i = next;
        match name.as_str() {
            "model" => caps.model = Some(body.trim().to_string()),
            "mccs_ver" => caps.mccs_version = Some(body.trim().to_string()),
            "vcp" => parse_vcp(&body, &mut caps.vcp),
            _ => {}
        }
    }
    caps
}

/// Content of the paren group starting at `open`, and the index after its closing `)`.
fn balanced(chars: &[char], open: usize) -> (String, usize) {
    let mut depth = 0usize;
    let mut out = String::new();
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '(' => {
                depth += 1;
                if depth > 1 {
                    out.push('(');
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return (out, i + 1);
                }
                out.push(')');
            }
            c => out.push(c),
        }
        i += 1;
    }
    (out, i)
}

fn parse_vcp(body: &str, out: &mut BTreeMap<u8, Vec<u8>>) {
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        while i < chars.len() && chars[i].is_ascii_hexdigit() {
            i += 1;
        }
        if start == i {
            i += 1; // whitespace or stray character
            continue;
        }
        let tok: String = chars[start..i].iter().collect();
        let Ok(code) = u8::from_str_radix(&tok, 16) else { continue };
        let mut j = i;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        let mut values = Vec::new();
        if j < chars.len() && chars[j] == '(' {
            let (inner, next) = balanced(&chars, j);
            values = top_level_hex(&inner);
            i = next;
        }
        out.insert(code, values);
    }
}

/// Hex bytes at nesting depth 0 (nested sub-feature groups are skipped).
fn top_level_hex(s: &str) -> Vec<u8> {
    let mut values = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut flush = |cur: &mut String| {
        if let Ok(v) = u8::from_str_radix(cur, 16) {
            values.push(v);
        }
        cur.clear();
    };
    for c in s.chars() {
        match c {
            '(' => {
                flush(&mut cur);
                depth += 1;
            }
            ')' => depth = depth.saturating_sub(1),
            c if c.is_ascii_hexdigit() && depth == 0 => cur.push(c),
            _ => flush(&mut cur),
        }
    }
    flush(&mut cur);
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typical_string() {
        let s = "(prot(monitor)type(lcd)model(27G2G3)cmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 12 14(01 05 08 0B) 16 18 1A 52 60(0F 11 12) 62 87 AC AE B2 B6 C0 C6 C8 C9 CA(01 02) CC(02 03 04 05 06 0A 0D) D6(01 04 05) DC(00 01 02 03 04) DF E0 E1 E2)mswhql(1)mccs_ver(2.2))";
        let c = parse(s);
        assert_eq!(c.model.as_deref(), Some("27G2G3"));
        assert_eq!(c.mccs_version.as_deref(), Some("2.2"));
        assert!(c.supports(0x10) && c.supports(0x12) && c.supports(0x62));
        assert_eq!(c.vcp[&0x14], vec![0x01, 0x05, 0x08, 0x0B]);
        assert_eq!(c.vcp[&0x60], vec![0x0F, 0x11, 0x12]);
        assert!(c.vcp[&0x10].is_empty());
        assert!(!c.supports(0x8D));
    }

    #[test]
    fn nested_subfeature_groups() {
        let c = parse("(vcp(10 DC(00 01 02 03 04) 14(01 05) 60(0F 11))model(X))");
        assert_eq!(c.vcp.len(), 4);
        assert_eq!(c.vcp[&0xDC], vec![0, 1, 2, 3, 4]);
        assert_eq!(c.model.as_deref(), Some("X"));
    }

    #[test]
    fn nested_values_with_subgroups() {
        let c = parse("(vcp(60(0F(01 02) 11)))");
        assert_eq!(c.vcp[&0x60], vec![0x0F, 0x11]);
    }

    #[test]
    fn no_outer_parens_and_garbage() {
        let c = parse("vcp(10 12)");
        assert!(c.supports(0x10) && c.supports(0x12));
        assert!(parse("").vcp.is_empty());
        assert!(parse("(((").vcp.is_empty());
    }
}

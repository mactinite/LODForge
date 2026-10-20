//! `lodforge plugin-dump`: records of a plugin with their subrecords (diagnostics for the plugins
//! DynDOLOD writes): header, then per record signature, flags, form id, EditorID and fields.

use std::path::PathBuf;

use anyhow::Result;

use lodforge_esp::{Entries, Entry, fields};

#[derive(clap::Args)]
pub struct Args {
    pub plugin: PathBuf,
    /// Only these record signatures (comma separated).
    #[arg(long, value_delimiter = ',')]
    pub sig: Vec<String>,
    /// At most this many records per signature.
    #[arg(long, default_value_t = 5)]
    pub limit: usize,
    /// Bytes of each field shown as hex.
    #[arg(long, default_value_t = 48)]
    pub bytes: usize,
    /// Show the group structure.
    #[arg(long)]
    pub groups: bool,
}

pub fn run(a: &Args) -> Result<()> {
    let pl = lodforge_esp::Plugin::open(&a.plugin)?;
    let h = &pl.header;
    println!("masters {:?}", h.masters);
    let raw = std::fs::read(&a.plugin)?;
    let tes4_len = u32::from_le_bytes(raw[4..8].try_into()?) as usize;
    println!("TES4 flags {:08X}", u32::from_le_bytes(raw[8..12].try_into()?));
    for f in fields(&raw[24..24 + tes4_len]) {
        println!("  {} {}", f.sig, show(f.data, a.bytes));
    }
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    walk(pl.top_entries(), a, &mut counts, 0)?;
    println!("records: {counts:?}");
    Ok(())
}

fn walk(es: Entries<'_>, a: &Args, counts: &mut std::collections::BTreeMap<String, usize>, depth: usize) -> Result<()> {
    for e in es {
        match e? {
            Entry::Record(r) => {
                let sig = r.sig.to_string();
                let n = counts.entry(sig.clone()).or_default();
                *n += 1;
                if (!a.sig.is_empty() && !a.sig.iter().any(|s| s.eq_ignore_ascii_case(&sig))) || *n > a.limit {
                    continue;
                }
                let data = r.data()?;
                println!("{}{sig} {:08X} flags {:08X}", "  ".repeat(depth), r.form_id, r.flags);
                for f in fields(&data) {
                    println!("{}  {} [{}] {}", "  ".repeat(depth), f.sig, f.data.len(), show(f.data, a.bytes));
                }
            }
            Entry::Group(g) => {
                if a.groups {
                    println!("{}GRUP type {} label {:08X}", "  ".repeat(depth), g.group_type, u32::from_le_bytes(g.label));
                }
                walk(g.entries(), a, counts, depth + usize::from(a.groups))?;
            }
        }
    }
    Ok(())
}

fn show(d: &[u8], n: usize) -> String {
    let text = d.len() > 1 && d[..d.len() - 1].iter().all(|&b| (0x20..0x7F).contains(&b)) && d[d.len() - 1] == 0;
    if text {
        return format!("\"{}\"", String::from_utf8_lossy(&d[..d.len() - 1]));
    }
    let hex: Vec<String> = d.iter().take(n).map(|b| format!("{b:02X}")).collect();
    format!("{}{}", hex.join(" "), if d.len() > n { " ..." } else { "" })
}

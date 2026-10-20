//! `lodforge info --json`: what a build can do for a profile: plugins, LOD worldspaces, seasons
//! with swap files. Used by the UI.

use std::time::Instant;

use anyhow::Result;

use crate::profile::ProfileArgs;

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long)]
    pub json: bool,
}

pub fn run(a: &Args) -> Result<()> {
    let t = Instant::now();
    let p = a.profile.open()?;
    let worlds = crate::build::lod_worlds(&p)?;
    let mut seasons: Vec<String> = Vec::new();
    for path in p.vfs.list_prefix("seasons\\") {
        let up = path.to_ascii_uppercase();
        for s in ["SPR", "SUM", "AUT", "WIN"] {
            if up.ends_with(&format!("{s}.INI")) && !seasons.iter().any(|x| x == s) {
                seasons.push(s.into());
            }
        }
    }
    seasons.sort_by_key(|s| ["SPR", "SUM", "AUT", "WIN"].iter().position(|x| x == s));
    if a.json {
        let q = |v: &[String]| v.iter().map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))).collect::<Vec<_>>().join(",");
        println!("{{\"plugins\":{},\"worlds\":[{}],\"seasons\":[{}],\"missing_archives\":{},\"ms\":{}}}", p.plugin_names.len(), q(&worlds), q(&seasons), p.missing_archives.len(), t.elapsed().as_millis());
    } else {
        println!("{} plugins; LOD worldspaces: {worlds:?}; seasons with swap files: {seasons:?}; {} missing archives; {} ms", p.plugin_names.len(), p.missing_archives.len(), t.elapsed().as_millis());
    }
    Ok(())
}

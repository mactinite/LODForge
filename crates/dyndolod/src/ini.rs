//! Minimal INI reader for DynDOLOD's configuration and rule files: `[Section]` headers,
//! `key=value` lines, `;` and `//` comments. Section and key lookups are case-insensitive; values
//! keep their order (rule files number their keys, but DynDOLOD reads them in file order).

#[derive(Debug, Clone, Default)]
pub struct Ini {
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, Default)]
pub struct Section {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
}

/// Decodes a config file: UTF-8 (BOM stripped) or Windows-1252-ish bytes.
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
                continue;
            }
            if line.starts_with('[') {
                if let Some(end) = line.find(']') {
                    ini.sections.push(Section { name: line[1..end].to_string(), entries: Vec::new() });
                }
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            if ini.sections.is_empty() {
                ini.sections.push(Section::default());
            }
            ini.sections.last_mut().unwrap().entries.push((k.trim().to_string(), v.trim().to_string()));
        }
        ini
    }

    pub fn read(path: &std::path::Path) -> std::io::Result<Ini> {
        Ok(Ini::parse(&decode(&std::fs::read(path)?)))
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// All sections with this name, in file order (rule files may repeat a section).
    pub fn sections_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Section> + 'a {
        self.sections.iter().filter(move |s| s.name.eq_ignore_ascii_case(name))
    }

    /// Sets `key` in `section` (appended, so it wins over earlier values in [Ini::get]).
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        self.sections.push(Section { name: section.to_string(), entries: vec![(key.to_string(), value.to_string())] });
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections.iter().filter(|s| s.name.eq_ignore_ascii_case(section)).flat_map(|s| s.entries.iter()).filter(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str()).last()
    }
}

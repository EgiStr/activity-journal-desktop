//! The engine's two TOML layers, read the way the engine reads them and written back without
//! losing comments. The form writes only `*.local.toml`, and only the values the owner changed, so
//! the files the engine loads stay the single source of truth.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

/// One configuration file pair: the tracked example and the git-ignored local override.
pub struct Layers {
    example: DocumentMut,
    local: DocumentMut,
    local_path: PathBuf,
}

impl Layers {
    /// Load `config/<stem>.example.toml` and `config/<stem>.local.toml`; either may be absent.
    pub fn load(engine_root: &Path, stem: &str) -> Result<Self, String> {
        let config = engine_root.join("config");
        Ok(Self {
            example: read_document(&config.join(format!("{stem}.example.toml")))?,
            local: read_document(&config.join(format!("{stem}.local.toml")))?,
            local_path: config.join(format!("{stem}.local.toml")),
        })
    }

    #[cfg(test)]
    fn from_text(example: &str, local: &str) -> Self {
        Self {
            example: example.parse().unwrap(),
            local: local.parse().unwrap(),
            local_path: PathBuf::from("unused.local.toml"),
        }
    }

    /// The effective value at `path`: local first, then example.
    fn get(&self, path: &[&str]) -> Option<&Item> {
        lookup(self.local.as_item(), path).or_else(|| lookup(self.example.as_item(), path))
    }

    pub fn get_str(&self, path: &[&str]) -> Option<String> {
        self.get(path).and_then(Item::as_str).map(str::to_owned)
    }

    pub fn get_bool(&self, path: &[&str]) -> Option<bool> {
        self.get(path).and_then(Item::as_bool)
    }

    /// Write `value` at `path` into the local layer, creating tables as needed.
    fn set(&mut self, path: &[&str], value: Item) {
        let (last, tables) = path.split_last().expect("a settings path is never empty");
        let mut table: &mut Table = self.local.as_table_mut();
        for name in tables {
            let entry = table.entry(name).or_insert_with(|| {
                let mut fresh = Table::new();
                fresh.set_implicit(true);
                Item::Table(fresh)
            });
            table = entry.as_table_mut().expect("a settings section is a table");
        }
        table[*last] = value;
    }

    pub fn set_str(&mut self, path: &[&str], value: &str) {
        if self.get_str(path).as_deref() != Some(value) {
            self.set(path, toml_edit::value(value));
        }
    }

    pub fn set_bool(&mut self, path: &[&str], value: bool) {
        if self.get_bool(path) != Some(value) {
            self.set(path, toml_edit::value(value));
        }
    }

    /// The local layer as it would be saved.
    pub fn local_text(&self) -> String {
        self.local.to_string()
    }

    pub fn save(&self) -> Result<(), String> {
        std::fs::write(&self.local_path, self.local_text())
            .map_err(|e| format!("cannot write {}: {e}", self.local_path.display()))
    }

    /// Plugin names listed in any stage. A local `stages` array replaces the example's.
    pub fn staged_plugins(&self) -> Vec<String> {
        let stages = self
            .local
            .get("stages")
            .or_else(|| self.example.get("stages"))
            .and_then(Item::as_array_of_tables);
        let mut names = Vec::new();
        for stage in stages.into_iter().flatten() {
            let plugins = stage.get("plugins").and_then(Item::as_array);
            for name in plugins.into_iter().flatten().filter_map(|v| v.as_str()) {
                names.push(name.to_owned());
            }
        }
        names
    }

    /// Whether the engine runs `plugin`; it does unless `enabled = false` is declared.
    pub fn plugin_enabled(&self, plugin: &str) -> bool {
        self.get_bool(&["plugins", plugin, "enabled"]) != Some(false)
    }
}

fn read_document(path: &Path) -> Result<DocumentMut, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .map_err(|e| format!("{} is not valid TOML: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn lookup<'a>(item: &'a Item, path: &[&str]) -> Option<&'a Item> {
    path.iter()
        .try_fold(item, |current, name| current.get(name))
        .filter(|i| !i.is_none())
}

/// A path from the settings, resolved the way the engine does: relative to its working directory.
pub fn resolve(engine_root: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        engine_root.join(path)
    }
}

/// The settings the form edits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsForm {
    pub data_dir: String,
    pub journal_dir: String,
    pub timezone: String,
    pub model: String,
    pub base_url: String,
    pub router_enabled: bool,
    pub privacy_enabled: bool,
}

const DATA_DIR: &[&str] = &["paths", "data_dir"];
const JOURNAL_DIR: &[&str] = &["paths", "journal_dir"];
const TIMEZONE: &[&str] = &["day", "timezone"];
const MODEL: &[&str] = &["router", "model"];
const BASE_URL: &[&str] = &["router", "base_url"];
const ROUTER_ENABLED: &[&str] = &["router", "enabled"];
const PRIVACY_ENABLED: &[&str] = &["privacy", "enabled"];

impl SettingsForm {
    pub fn from_layers(layers: &Layers) -> Self {
        let text = |path| layers.get_str(path).unwrap_or_default();
        Self {
            data_dir: text(DATA_DIR),
            journal_dir: text(JOURNAL_DIR),
            timezone: text(TIMEZONE),
            model: text(MODEL),
            base_url: text(BASE_URL),
            router_enabled: layers.get_bool(ROUTER_ENABLED).unwrap_or(true),
            privacy_enabled: layers.get_bool(PRIVACY_ENABLED).unwrap_or(false),
        }
    }

    /// Every reason the engine would refuse these values; empty means safe to save.
    pub fn problems(&self, engine_root: &Path) -> Vec<String> {
        let mut problems = Vec::new();
        let data = resolve(engine_root, &self.data_dir);
        if self.data_dir.trim().is_empty() {
            problems.push("Data directory is empty.".to_owned());
        } else if !data.is_dir() && !data.parent().is_some_and(Path::is_dir) {
            problems.push(format!(
                "Data directory {} and its parent do not exist.",
                data.display()
            ));
        }
        let journal = resolve(engine_root, &self.journal_dir);
        if self.journal_dir.trim().is_empty() {
            problems.push("Journal directory is empty.".to_owned());
        } else if !journal.is_dir() {
            problems.push(format!("Journal directory {} does not exist.", journal.display()));
        }
        if self.timezone.parse::<chrono_tz::Tz>().is_err() {
            problems.push(format!(
                "\"{}\" is not an IANA timezone such as Asia/Jakarta.",
                self.timezone
            ));
        }
        if self.model.trim().is_empty() {
            problems.push("Model is empty.".to_owned());
        }
        if !(self.base_url.starts_with("http://") || self.base_url.starts_with("https://")) {
            problems.push("Router URL must start with http:// or https://.".to_owned());
        }
        problems
    }

    /// Copy the changed values into the local layer; unchanged values are not written.
    pub fn apply(&self, layers: &mut Layers) {
        layers.set_str(DATA_DIR, &self.data_dir);
        layers.set_str(JOURNAL_DIR, &self.journal_dir);
        layers.set_str(TIMEZONE, &self.timezone);
        layers.set_str(MODEL, &self.model);
        layers.set_str(BASE_URL, &self.base_url);
        layers.set_bool(ROUTER_ENABLED, self.router_enabled);
        layers.set_bool(PRIVACY_ENABLED, self.privacy_enabled);
    }
}

/// A plugin found on disk through its manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginEntry {
    pub name: String,
    pub kind: String,
    pub description: String,
}

/// Every `plugins/<folder>/plugin.json`, sorted by name. Never a hardcoded list.
pub fn discover_plugins(engine_root: &Path) -> Vec<PluginEntry> {
    let Ok(folders) = std::fs::read_dir(engine_root.join("plugins")) else {
        return Vec::new();
    };
    let mut plugins: Vec<PluginEntry> = folders
        .filter_map(Result::ok)
        .filter_map(|folder| std::fs::read_to_string(folder.path().join("plugin.json")).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter_map(|manifest| {
            let field = |name: &str| {
                manifest
                    .get(name)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            let name = field("name");
            (!name.is_empty()).then(|| PluginEntry {
                name,
                kind: field("kind"),
                description: field("description"),
            })
        })
        .collect();
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "[paths]\ndata_dir = \"./data\"\njournal_dir = \"./journal\"\n\n[day]\ntimezone = \"Asia/Jakarta\"\n\n[router]\nmodel = \"m\"\nbase_url = \"http://localhost:1/v1\"\nenabled = true\n";

    #[test]
    fn local_overrides_example() {
        let layers = Layers::from_text(EXAMPLE, "[day]\ntimezone = \"UTC\"\n");
        assert_eq!(layers.get_str(TIMEZONE).as_deref(), Some("UTC"));
        assert_eq!(layers.get_str(DATA_DIR).as_deref(), Some("./data"));
    }

    #[test]
    fn saving_writes_only_changes_and_keeps_comments() {
        let local = "# my note\n[day]\ntimezone = \"UTC\" # keep me\n";
        let mut layers = Layers::from_text(EXAMPLE, local);
        let mut form = SettingsForm::from_layers(&layers);
        form.model = "other".to_owned();
        form.apply(&mut layers);
        let saved = layers.local_text();
        assert!(
            saved.starts_with("# my note\n[day]\ntimezone = \"UTC\" # keep me\n"),
            "{saved}"
        );
        assert!(saved.contains("model = \"other\""), "{saved}");
        assert!(
            !saved.contains("data_dir"),
            "unchanged values must not be copied: {saved}"
        );
    }

    #[test]
    fn plugin_tables_are_written_as_dotted_headers() {
        let mut layers = Layers::from_text(EXAMPLE, "");
        layers.set_bool(&["plugins", "chrome-history-collector", "enabled"], false);
        assert_eq!(
            layers.local_text().trim(),
            "[plugins.chrome-history-collector]\nenabled = false"
        );
        assert!(!layers.plugin_enabled("chrome-history-collector"));
        assert!(layers.plugin_enabled("anything-else"));
    }

    #[test]
    fn local_stages_replace_example_stages() {
        let example = "[[stages]]\nkind = \"collector\"\nplugins = [\"a\", \"b\"]\n";
        assert_eq!(Layers::from_text(example, "").staged_plugins(), ["a", "b"]);
        let local = "[[stages]]\nkind = \"collector\"\nplugins = [\"c\"]\n";
        assert_eq!(Layers::from_text(example, local).staged_plugins(), ["c"]);
    }

    #[test]
    fn invalid_values_are_refused_in_the_form() {
        let root = std::env::temp_dir();
        let form = SettingsForm {
            data_dir: "./x/y/z/missing".to_owned(),
            journal_dir: "./missing-journal-dir".to_owned(),
            timezone: "Mars/Olympus".to_owned(),
            model: " ".to_owned(),
            base_url: "localhost".to_owned(),
            ..SettingsForm::default()
        };
        assert_eq!(form.problems(&root).len(), 5);
        let good = SettingsForm {
            data_dir: "./data".to_owned(),
            journal_dir: ".".to_owned(),
            timezone: "Asia/Jakarta".to_owned(),
            model: "m".to_owned(),
            base_url: "http://localhost:1/v1".to_owned(),
            ..SettingsForm::default()
        };
        assert_eq!(good.problems(&root), Vec::<String>::new());
    }
}

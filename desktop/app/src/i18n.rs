use std::collections::BTreeMap;
#[derive(Clone)]
pub struct Language {
    pub id: String,
    pub name: String,
    pub messages: BTreeMap<String, String>,
}
pub struct I18n {
    pub languages: Vec<Language>,
    pub selected: usize,
}
impl Default for I18n {
    fn default() -> Self {
        let mut s = Self {
            languages: vec![],
            selected: 0,
        };
        s.register("en", "English", include_str!("../locales/en.json"));
        s.register("zh-CN", "简体中文", include_str!("../locales/zh-CN.json"));
        if let Ok(dir) = std::env::var("PDBG_LOCALES") {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|x| x == "json") {
                        if let (Some(id), Ok(data)) = (
                            path.file_stem().and_then(|x| x.to_str()),
                            std::fs::read_to_string(&path),
                        ) {
                            s.register(id, id, &data);
                        }
                    }
                }
            }
        }
        s
    }
}
impl I18n {
    pub fn register(&mut self, id: &str, name: &str, json: &str) {
        if let Ok(messages) = serde_json::from_str::<BTreeMap<String, String>>(json) {
            let language = Language {
                id: id.into(),
                name: messages
                    .get("language_name")
                    .cloned()
                    .unwrap_or(name.into()),
                messages,
            };
            if let Some(i) = self.languages.iter().position(|l| l.id == id) {
                self.languages[i] = language;
            } else {
                self.languages.push(language);
            }
        }
    }
    pub fn tr<'a>(&'a self, key: &'a str) -> &'a str {
        self.languages[self.selected]
            .messages
            .get(key)
            .or_else(|| self.languages[0].messages.get(key))
            .map(String::as_str)
            .unwrap_or(key)
    }
}

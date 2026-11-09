//! A small, profile-local list of pages opened in the browser.
use std::path::PathBuf;

const LIMIT: usize = 12;

pub(super) fn shared(root: PathBuf) -> std::rc::Rc<std::cell::RefCell<Recent>> {
    use std::{
        cell::RefCell,
        collections::HashMap,
        rc::{Rc, Weak},
    };
    thread_local! {
        static STORES: RefCell<HashMap<PathBuf, Weak<RefCell<Recent>>>> = RefCell::new(HashMap::new());
    }
    STORES.with(|stores| {
        let mut stores = stores.borrow_mut();
        if let Some(existing) = stores.get(&root).and_then(Weak::upgrade) {
            return existing;
        }
        let recent = Rc::new(RefCell::new(Recent::for_profile(root.clone())));
        stores.insert(root, Rc::downgrade(&recent));
        recent
    })
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(super) struct Visit {
    pub url: String,
    pub title: String,
}

impl Visit {
    pub fn display(&self) -> (String, String) {
        let Ok(url) = url::Url::parse(&self.url) else {
            return (self.title.clone(), String::new());
        };
        let host = url.host_str().unwrap_or("").to_string();
        if matches!(host.as_str(), "duckduckgo.com" | "www.duckduckgo.com") {
            if let Some((_, query)) = url.query_pairs().find(|(key, _)| key == "q") {
                if !query.trim().is_empty() {
                    return (query.into_owned(), "DuckDuckGo search".into());
                }
            }
        }
        let title = self.title.trim();
        (
            if title.is_empty() {
                host.clone()
            } else {
                title.to_string()
            },
            host,
        )
    }
}

#[derive(Default)]
pub(super) struct Recent {
    path: Option<PathBuf>,
    pub visits: Vec<Visit>,
    icons: std::collections::HashMap<String, std::sync::Arc<gpui::Image>>,
}

impl Recent {
    pub fn for_profile(root: PathBuf) -> Self {
        let path = root.join("recent-pages.json");
        let visits: Vec<Visit> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut recent = Self {
            path: Some(path),
            visits: visits
                .into_iter()
                .filter(|v| super::model::allowed_navigation(&v.url))
                .take(LIMIT)
                .collect(),
            icons: Default::default(),
        };
        for visit in &recent.visits {
            let Some(path) = recent.icon_path(&visit.url) else {
                continue;
            };
            if let Ok(bytes) = std::fs::read(path) {
                if bytes.len() <= 1024 * 1024 {
                    recent.icons.insert(
                        visit.url.clone(),
                        std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes)),
                    );
                }
            }
        }
        recent
    }

    pub fn record(&mut self, url: &str, title: &str) {
        if !super::model::allowed_navigation(url) {
            return;
        }
        self.visits.retain(|entry| entry.url != url);
        self.visits.insert(
            0,
            Visit {
                url: url.to_owned(),
                title: title.chars().take(200).collect(),
            },
        );
        while self.visits.len() > LIMIT {
            if let Some(old) = self.visits.pop() {
                self.icons.remove(&old.url);
                if let Some(path) = self.icon_path(&old.url) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        self.save();
    }

    fn icon_path(&self, url: &str) -> Option<PathBuf> {
        use sha2::{Digest, Sha256};
        Some(
            self.path
                .as_ref()?
                .parent()?
                .join("favicons")
                .join(format!("{:x}.png", Sha256::digest(url.as_bytes()))),
        )
    }

    pub fn icon(&self, url: &str) -> Option<std::sync::Arc<gpui::Image>> {
        self.icons.get(url).cloned()
    }

    pub fn set_icon(&mut self, url: &str, bytes: Vec<u8>) {
        if !self.visits.iter().any(|visit| visit.url == url) {
            return;
        }
        if let Some(path) = self.icon_path(url) {
            let result = std::fs::create_dir_all(path.parent().unwrap())
                .and_then(|_| std::fs::write(path, &bytes));
            if let Err(error) = result {
                tracing::warn!(%error, "could not save browser favicon");
            }
        }
        self.icons.insert(
            url.to_string(),
            std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes)),
        );
    }

    pub fn remove(&mut self, url: &str) {
        self.icons.remove(url);
        if let Some(path) = self.icon_path(url) {
            let _ = std::fs::remove_file(path);
        }
        self.visits.retain(|entry| entry.url != url);
        self.save();
    }

    pub fn clear(&mut self) {
        let urls: Vec<String> = self.visits.iter().map(|visit| visit.url.clone()).collect();
        for url in urls {
            if let Some(path) = self.icon_path(&url) {
                let _ = std::fs::remove_file(path);
            }
        }
        self.icons.clear();
        self.visits.clear();
        self.save();
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            let bytes = serde_json::to_vec(&self.visits)?;
            std::fs::write(&tmp, bytes)?;
            if let Err(error) = std::fs::rename(&tmp, path) {
                let _ = std::fs::remove_file(tmp);
                return Err(error);
            }
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "could not save browser recent pages");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_is_bounded_deduplicated_and_persistent() {
        let dir = tempfile::tempdir().unwrap();
        let mut recent = Recent::for_profile(dir.path().into());
        for index in 0..15 {
            recent.record(&format!("https://example.com/{index}"), "Example");
        }
        recent.record("https://example.com/4", "New title");
        assert_eq!(recent.visits.len(), LIMIT);
        assert_eq!(recent.visits[0].title, "New title");
        assert_eq!(Recent::for_profile(dir.path().into()).visits, recent.visits);
        recent.remove("https://example.com/4");
        assert!(!recent.visits.iter().any(|v| v.url.ends_with("/4")));
        recent.clear();
        assert!(Recent::for_profile(dir.path().into()).visits.is_empty());
    }
    #[test]
    fn favicon_is_restored_and_removed_with_its_visit() {
        let dir = tempfile::tempdir().unwrap();
        let mut recent = Recent::for_profile(dir.path().into());
        let url = "https://example.com/";
        recent.record(url, "Example");
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        recent.set_icon(url, png.into_inner());
        let saved = std::fs::read(recent.icon_path(url).unwrap()).unwrap();
        assert!(image::load_from_memory(&saved).is_ok());
        assert!(Recent::for_profile(dir.path().into()).icon(url).is_some());
        recent.remove(url);
        assert!(Recent::for_profile(dir.path().into()).icon(url).is_none());
    }
    #[test]
    fn search_query_is_readable() {
        let visit = Visit {
            url: "https://duckduckgo.com/?q=what+is+html&ia=web".into(),
            title: String::new(),
        };
        assert_eq!(
            visit.display(),
            ("what is html".into(), "DuckDuckGo search".into())
        );
    }
}

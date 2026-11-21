use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::vscode::{
    CompileOptions, DetectedThemeSource, ImportReport, SourceCompilation, compile_source,
};
use crate::{ThemeFamily, ThemeRegistry, replace_custom_families};

const LIBRARY_FILE: &str = "theme-library.json";
const MAX_LIBRARY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LIBRARY_ENTRIES: usize = 256;
const MAX_LIBRARY_VARIANTS: usize = 1024;
const MAX_THEME_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PACKAGE_ENTRIES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Snapshot,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CustomThemeSource {
    ImportedSnapshot {
        imported_from: Option<PathBuf>,
    },
    LinkedFile {
        path: PathBuf,
    },
    LinkedPackage {
        path: PathBuf,
    },
    /// A native resolved-family file created by Zuse for direct editing.
    EditableFile {
        path: PathBuf,
    },
}

impl CustomThemeSource {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::ImportedSnapshot { imported_from } => imported_from.as_deref(),
            Self::LinkedFile { path }
            | Self::LinkedPackage { path }
            | Self::EditableFile { path } => Some(path),
        }
    }

    pub fn is_linked(&self) -> bool {
        matches!(
            self,
            Self::LinkedFile { .. } | Self::LinkedPackage { .. } | Self::EditableFile { .. }
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::ImportedSnapshot { .. } => "Imported",
            Self::LinkedFile { .. } => "Linked file",
            Self::LinkedPackage { .. } => "Linked package",
            Self::EditableFile { .. } => "Editable file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CustomThemeStatus {
    Ready,
    Warning { message: String },
}

impl Default for CustomThemeStatus {
    fn default() -> Self {
        Self::Ready
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomThemeEntry {
    pub id: String,
    pub name: String,
    pub source: CustomThemeSource,
    pub family: ThemeFamily,
    pub reports: BTreeMap<String, ImportReport>,
    pub selected_variant_ids: Vec<String>,
    #[serde(default)]
    pub status: CustomThemeStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomThemeLibrary {
    #[serde(default)]
    pub entries: Vec<CustomThemeEntry>,
}

impl CustomThemeLibrary {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(LIBRARY_FILE)
    }

    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = Self::path(data_dir);
        let backup = backup_path(&path);
        let load_path = if path.exists() { &path } else { &backup };
        let source = match read_library_file(load_path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", path.display()));
            }
        };
        let library: Self = serde_json::from_str(&source)
            .with_context(|| format!("could not parse {}", load_path.display()))?;
        library.validate_capacity()?;
        Ok(library)
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        self.validate_capacity()?;
        fs::create_dir_all(data_dir)
            .with_context(|| format!("could not create {}", data_dir.display()))?;
        let path = Self::path(data_dir);
        let encoded = serde_json::to_vec_pretty(self)?;
        if encoded.len() as u64 > MAX_LIBRARY_BYTES {
            bail!("custom theme library exceeds the {MAX_LIBRARY_BYTES}-byte limit");
        }
        replace_file_recoverably(&path, &encoded)
    }

    pub fn install_runtime(&self) {
        replace_custom_families(
            self.entries
                .iter()
                .map(|entry| entry.family.clone())
                .collect(),
        );
    }

    pub fn entry(&self, id: &str) -> Option<&CustomThemeEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn compile(path: &Path, family_id: &str, family_name: &str) -> Result<SourceCompilation> {
        let options = CompileOptions {
            family_id: family_id.into(),
            family_name: family_name.into(),
            source_url: path.display().to_string(),
            revision: "local".into(),
            license: "User supplied".into(),
        };
        if path.is_file() && path.file_name().is_some_and(|name| name != "package.json") {
            let file = fs::File::open(path)?;
            let mut source = String::new();
            file.take(MAX_THEME_JSON_BYTES + 1)
                .read_to_string(&mut source)?;
            if source.len() as u64 > MAX_THEME_JSON_BYTES {
                bail!("theme JSON exceeds the {MAX_THEME_JSON_BYTES}-byte limit");
            }
            if let Ok(value) = json5::from_str(&source)
                && let Some(compilation) =
                    compile_own_format(&value, &path.canonicalize()?, &options.family_id)?
            {
                return Ok(compilation);
            }
        }
        compile_source(path, options)
    }

    pub fn compile_pasted(source: &str) -> Result<SourceCompilation> {
        if source.len() as u64 > MAX_THEME_JSON_BYTES {
            bail!("theme JSON exceeds the {MAX_THEME_JSON_BYTES}-byte limit");
        }
        let source = source.trim().trim_start_matches('\u{feff}').trim();
        let source = strip_code_fence(source).unwrap_or(source);
        let value: serde_json::Value = json5::from_str(source).context("invalid theme JSON")?;
        if let Some(compilation) = compile_own_format(&value, Path::new(""), "custom-theme")? {
            return Ok(compilation);
        }
        let has_workbench_colors = value
            .get("colors")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|colors| colors.keys().any(|key| key.contains('.')));
        if !has_workbench_colors
            && value.get("tokenColors").is_none()
            && value.get("semanticTokenColors").is_none()
        {
            bail!("expected Zuse, T3 Code, or VS Code theme JSON");
        }
        let scratch = tempfile::tempdir()?;
        let path = scratch.path().join("pasted-theme.json");
        fs::write(&path, source)?;
        let mut compilation = compile_source(
            &path,
            CompileOptions {
                family_id: "custom-theme".into(),
                family_name: "Pasted theme".into(),
                source_url: "clipboard".into(),
                revision: "local".into(),
                license: "User supplied".into(),
            },
        )?;
        compilation.family.name = compilation.family.variants[0].name.clone();
        compilation.path.clear();
        for report in compilation.reports.values_mut() {
            report.source_files = vec!["clipboard".into()];
        }
        Ok(compilation)
    }

    pub fn compile_community_package(
        bytes: &[u8],
        options: CompileOptions,
    ) -> Result<SourceCompilation> {
        if bytes.len() as u64 > MAX_PACKAGE_BYTES {
            bail!("theme package exceeds the download limit");
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        if archive.len() > MAX_PACKAGE_ENTRIES {
            bail!("theme package contains too many files");
        }
        // Index every extractable theme-format file under `extension/`;
        // nothing outside it, nothing in `node_modules`, no traversal.
        let mut index = BTreeMap::new();
        let mut seen = HashSet::new();
        for position in 0..archive.len() {
            let entry = archive.by_index(position)?;
            if !entry.is_file() || entry.is_symlink() {
                continue;
            }
            let Some(relative) = entry
                .enclosed_name()
                .and_then(|path| path.strip_prefix("extension").ok().map(Path::to_path_buf))
            else {
                continue;
            };
            let normal = relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)));
            let vendored = relative
                .components()
                .any(|part| part.as_os_str() == "node_modules");
            let extension = relative
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default();
            if !normal
                || vendored
                || !["json", "jsonc", "tmTheme", "plist"]
                    .iter()
                    .any(|ext| ext.eq_ignore_ascii_case(extension))
            {
                continue;
            }
            if !seen.insert(relative.to_string_lossy().to_lowercase()) {
                bail!("theme package contains duplicate paths");
            }
            index.insert(relative, position);
        }
        // Extract the manifest, the themes it declares, and whatever those
        // files `include` or load `tokenColors` from, wherever they sit in the
        // package. Unrelated data never counts against the limits. Without a
        // readable manifest, fall back to every indexed file.
        let mut queue = std::collections::VecDeque::from(
            declared_theme_paths(&mut archive).unwrap_or_else(|| index.keys().cloned().collect()),
        );
        queue.push_front(PathBuf::from("package.json"));
        let scratch = tempfile::tempdir()?;
        let root = scratch.path().canonicalize()?;
        let mut extracted = HashSet::new();
        let mut available = MAX_PACKAGE_BYTES;
        while let Some(relative) = queue.pop_front() {
            let Some(&position) = index.get(&relative) else {
                continue;
            };
            if !extracted.insert(relative.clone()) {
                continue;
            }
            let limit = available.min(MAX_THEME_JSON_BYTES);
            let entry = archive.by_index(position)?;
            if entry.size() > limit {
                bail!("theme package exceeds the extracted data limit");
            }
            let mut contents = Vec::new();
            entry.take(limit + 1).read_to_end(&mut contents)?;
            if contents.len() as u64 > limit {
                bail!("theme package exceeds the extracted data limit");
            }
            available -= contents.len() as u64;
            let destination = root.join(&relative);
            fs::create_dir_all(destination.parent().context("theme file has no parent")?)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?
                .write_all(&contents)?;
            if relative != Path::new("package.json") {
                queue.extend(theme_dependencies(&relative, &contents));
            }
        }
        let manifest = root.join("package.json");
        if !manifest.is_file() {
            bail!("theme package is missing its manifest");
        }
        let mut compilation = compile_source(&manifest, options)?;
        compilation.path.clear();
        for failure in &mut compilation.failures {
            if let Ok(relative) = failure.path.strip_prefix(&root) {
                failure.path = relative.to_path_buf();
            }
        }
        for report in compilation.reports.values_mut() {
            for source in &mut report.source_files {
                if let Ok(relative) = Path::new(source).strip_prefix(&root) {
                    *source = relative.to_string_lossy().into_owned();
                }
            }
        }
        Ok(compilation)
    }

    pub fn install(
        &mut self,
        compilation: SourceCompilation,
        selected_variant_ids: &[String],
        mode: InstallMode,
    ) -> Result<String> {
        let selected: HashSet<_> = selected_variant_ids.iter().collect();
        let mut family = compilation.family;
        family
            .variants
            .retain(|variant| selected.is_empty() || selected.contains(&variant.id));
        if family.variants.is_empty() {
            bail!("select at least one successfully compiled variant");
        }
        self.ensure_capacity_for(family.variants.len())?;
        let entry_id = unique_id(
            &family.id,
            self.entries.iter().map(|entry| entry.id.as_str()),
        );
        let old_variant_ids = family
            .variants
            .iter()
            .map(|variant| variant.id.clone())
            .collect::<Vec<_>>();
        if entry_id != family.id {
            rekey_family(&mut family, &entry_id);
        }
        let errors = validation_errors(&family);
        if !errors.is_empty() {
            bail!("theme validation failed: {}", errors.join("; "));
        }
        let selected_variant_ids = family
            .variants
            .iter()
            .map(|variant| variant.id.clone())
            .collect::<Vec<_>>();
        let mut reports = BTreeMap::new();
        for (old_id, variant) in old_variant_ids.iter().zip(&family.variants) {
            if let Some(report) = compilation.reports.get(old_id) {
                reports.insert(variant.id.clone(), report.clone());
            }
        }
        let path = compilation.path;
        if mode == InstallMode::Link && path.as_os_str().is_empty() {
            bail!("only local sources can be linked");
        }
        let source = match (mode, compilation.source_kind) {
            (InstallMode::Snapshot, _) => CustomThemeSource::ImportedSnapshot {
                imported_from: (!path.as_os_str().is_empty()).then_some(path),
            },
            (InstallMode::Link, DetectedThemeSource::File) => {
                CustomThemeSource::LinkedFile { path }
            }
            (InstallMode::Link, DetectedThemeSource::Package) => {
                CustomThemeSource::LinkedPackage { path }
            }
        };
        self.entries.push(CustomThemeEntry {
            id: entry_id.clone(),
            name: family.name.clone(),
            source,
            family,
            reports,
            selected_variant_ids,
            status: CustomThemeStatus::Ready,
        });
        self.validate_capacity()?;
        Ok(entry_id)
    }

    /// Reload a linked source. The existing compiled family is only replaced
    /// after a complete successful compile, preserving the last known good
    /// version on any source error.
    pub fn reload(&mut self, id: &str) -> Result<()> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| anyhow!("unknown custom theme `{id}`"))?;
        let path = match &entry.source {
            CustomThemeSource::LinkedFile { path } | CustomThemeSource::LinkedPackage { path } => {
                path.clone()
            }
            CustomThemeSource::EditableFile { path } => {
                let path = path.clone();
                let family = match load_editable_family(&path, &entry.id) {
                    Ok(family) => family,
                    Err(error) => {
                        entry.status = CustomThemeStatus::Warning {
                            message: error.to_string(),
                        };
                        return Err(error);
                    }
                };
                entry.selected_variant_ids = family
                    .variants
                    .iter()
                    .map(|variant| variant.id.clone())
                    .collect();
                entry.name = family.name.clone();
                entry.family = family;
                entry.reports.clear();
                entry.status = CustomThemeStatus::Ready;
                return Ok(());
            }
            CustomThemeSource::ImportedSnapshot { .. } => bail!("imported snapshots cannot reload"),
        };
        let wanted: HashSet<_> = entry.selected_variant_ids.iter().cloned().collect();
        let compilation = match Self::compile(&path, &entry.id, &entry.name) {
            Ok(compilation) => {
                let selected_failures = compilation
                    .failures
                    .iter()
                    .filter(|failure| wanted.contains(&failure.id))
                    .map(|failure| format!("{}: {}", failure.name, failure.message))
                    .collect::<Vec<_>>();
                if selected_failures.is_empty() {
                    compilation
                } else {
                    let message = selected_failures.join("; ");
                    entry.status = CustomThemeStatus::Warning {
                        message: message.clone(),
                    };
                    return Err(anyhow!(message));
                }
            }
            Err(error) => {
                entry.status = CustomThemeStatus::Warning {
                    message: error.to_string(),
                };
                return Err(error);
            }
        };
        let mut family = compilation.family;
        family
            .variants
            .retain(|variant| wanted.contains(&variant.id));
        if family.variants.is_empty() {
            let message = "the linked source no longer contains any selected variants".to_string();
            entry.status = CustomThemeStatus::Warning {
                message: message.clone(),
            };
            bail!(message);
        }
        let errors = validation_errors(&family);
        if !errors.is_empty() {
            let message = format!("theme validation failed: {}", errors.join("; "));
            entry.status = CustomThemeStatus::Warning {
                message: message.clone(),
            };
            bail!(message);
        }
        entry.family = family;
        entry.reports = compilation
            .reports
            .into_iter()
            .filter(|(variant_id, _)| wanted.contains(variant_id))
            .collect();
        entry.status = CustomThemeStatus::Ready;
        Ok(())
    }

    pub fn unlink(&mut self, id: &str) -> Result<()> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| anyhow!("unknown custom theme `{id}`"))?;
        let imported_from = entry.source.path().map(Path::to_path_buf);
        entry.source = CustomThemeSource::ImportedSnapshot { imported_from };
        entry.status = CustomThemeStatus::Ready;
        Ok(())
    }

    pub fn duplicate_as_snapshot(&mut self, id: &str) -> Result<String> {
        let original = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown custom theme `{id}`"))?;
        self.ensure_capacity_for(original.family.variants.len())?;
        let new_id = unique_id(
            &format!("{}-copy", original.id),
            self.entries.iter().map(|entry| entry.id.as_str()),
        );
        let mut duplicate = original;
        duplicate.id = new_id.clone();
        duplicate.name = format!("{} Copy", duplicate.name);
        let old_variant_ids = duplicate
            .family
            .variants
            .iter()
            .map(|variant| variant.id.clone())
            .collect::<Vec<_>>();
        rekey_family(&mut duplicate.family, &new_id);
        duplicate.reports = old_variant_ids
            .iter()
            .zip(&duplicate.family.variants)
            .filter_map(|(old_id, variant)| {
                duplicate
                    .reports
                    .get(old_id)
                    .cloned()
                    .map(|report| (variant.id.clone(), report))
            })
            .collect();
        duplicate.selected_variant_ids = duplicate
            .family
            .variants
            .iter()
            .map(|variant| variant.id.clone())
            .collect();
        duplicate.source = CustomThemeSource::ImportedSnapshot {
            imported_from: duplicate.source.path().map(Path::to_path_buf),
        };
        duplicate.status = CustomThemeStatus::Ready;
        self.entries.push(duplicate);
        Ok(new_id)
    }

    /// Duplicate a compiled family into a native, user-editable file. Reveal
    /// opens that file in the system file manager; Reload validates it and
    /// preserves the last known good family if an edit is invalid.
    pub fn duplicate_as_editable(&mut self, id: &str, data_dir: &Path) -> Result<String> {
        let mut family = self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.family.clone())
            .or_else(|| {
                ThemeRegistry::builtin()
                    .families
                    .iter()
                    .find(|family| family.id == id)
                    .cloned()
            })
            .ok_or_else(|| anyhow!("unknown custom theme `{id}`"))?;
        family.name = format!("{} Copy", family.name);
        self.create_editable(family, data_dir)
    }

    pub fn create_editable(&mut self, mut family: ThemeFamily, data_dir: &Path) -> Result<String> {
        if family.id.is_empty()
            || !family
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            bail!("invalid editable theme identifier");
        }
        self.ensure_capacity_for(family.variants.len())?;
        let editable_dir = data_dir.join("custom-themes");
        fs::create_dir_all(&editable_dir)
            .with_context(|| format!("could not create {}", editable_dir.display()))?;
        let used_files = fs::read_dir(&editable_dir)?
            .filter_map(Result::ok)
            .filter_map(|file| file.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(".zuse-theme.json").map(str::to_owned))
            .collect::<Vec<_>>();
        let new_id = unique_id(
            &format!("{}-copy", family.id),
            self.entries
                .iter()
                .map(|entry| entry.id.as_str())
                .chain(used_files.iter().map(String::as_str)),
        );
        rekey_family(&mut family, &new_id);
        let errors = validation_errors(&family);
        if !errors.is_empty() {
            bail!("theme validation failed: {}", errors.join("; "));
        }

        let path = editable_dir.join(format!("{new_id}.zuse-theme.json"));
        replace_file_recoverably(&path, &serde_json::to_vec_pretty(&family)?)?;

        let selected_variant_ids = family
            .variants
            .iter()
            .map(|variant| variant.id.clone())
            .collect();
        self.entries.push(CustomThemeEntry {
            id: new_id.clone(),
            name: family.name.clone(),
            source: CustomThemeSource::EditableFile { path },
            family,
            reports: BTreeMap::new(),
            selected_variant_ids,
            status: CustomThemeStatus::Ready,
        });
        Ok(new_id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.id != id);
        self.entries.len() != before
    }

    fn validate_capacity(&self) -> Result<()> {
        if self.entries.len() > MAX_LIBRARY_ENTRIES {
            bail!("custom theme library is limited to {MAX_LIBRARY_ENTRIES} entries");
        }
        let variants = self
            .entries
            .iter()
            .try_fold(0usize, |count, entry| {
                count.checked_add(entry.family.variants.len())
            })
            .ok_or_else(|| anyhow!("custom theme variant count overflow"))?;
        if variants > MAX_LIBRARY_VARIANTS {
            bail!("custom theme library is limited to {MAX_LIBRARY_VARIANTS} variants");
        }
        Ok(())
    }

    fn ensure_capacity_for(&self, additional_variants: usize) -> Result<()> {
        if self.entries.len() >= MAX_LIBRARY_ENTRIES {
            bail!("custom theme library is limited to {MAX_LIBRARY_ENTRIES} entries");
        }
        let current = self
            .entries
            .iter()
            .try_fold(0usize, |count, entry| {
                count.checked_add(entry.family.variants.len())
            })
            .ok_or_else(|| anyhow!("custom theme variant count overflow"))?;
        if current
            .checked_add(additional_variants)
            .is_none_or(|total| total > MAX_LIBRARY_VARIANTS)
        {
            bail!("custom theme library is limited to {MAX_LIBRARY_VARIANTS} variants");
        }
        Ok(())
    }
}

/// The theme files `extension/package.json` contributes, relative to the
/// extension root, or `None` when the manifest is missing or unreadable.
fn declared_theme_paths<R: io::Read + io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Option<Vec<PathBuf>> {
    let entry = archive.by_name("extension/package.json").ok()?;
    let mut manifest = String::new();
    entry
        .take(MAX_THEME_JSON_BYTES)
        .read_to_string(&mut manifest)
        .ok()?;
    let manifest: serde_json::Value = json5::from_str(&manifest).ok()?;
    let paths = manifest["contributes"]["themes"]
        .as_array()?
        .iter()
        .filter_map(|theme| package_relative(Path::new(""), theme["path"].as_str()?))
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some(paths)
}

/// Files a theme loads by relative path: its `include` parent and a
/// `tokenColors` file reference.
fn theme_dependencies(theme: &Path, contents: &[u8]) -> Vec<PathBuf> {
    let Ok(text) = std::str::from_utf8(contents) else {
        return Vec::new();
    };
    let Ok(value) = json5::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let base = theme.parent().unwrap_or(Path::new(""));
    ["include", "tokenColors"]
        .into_iter()
        .filter_map(|key| package_relative(base, value.get(key)?.as_str()?))
        .collect()
}

/// `reference` resolved against `base` inside the package; `None` if it
/// climbs out of the extension root or is absolute.
fn package_relative(base: &Path, reference: &str) -> Option<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in base.join(reference).components() {
        match component {
            std::path::Component::Normal(part) => resolved.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    (!resolved.as_os_str().is_empty()).then_some(resolved)
}

/// The body of a Markdown code fence (``` or ~~~, any length of at least
/// three, optional info string), when the whole input is one. The closing
/// fence must use the opener's character, at least as many times, with only
/// whitespace after it.
fn strip_code_fence(source: &str) -> Option<&str> {
    let marker = source.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let opener = source.chars().take_while(|c| *c == marker).count();
    if opener < 3 {
        return None;
    }
    let (_, rest) = source.split_once('\n')?;
    let (body, closing) = rest
        .trim_end()
        .rsplit_once('\n')
        .unwrap_or(("", rest.trim_end()));
    let closing = closing.trim();
    let closes = closing.chars().all(|c| c == marker) && closing.chars().count() >= opener;
    closes.then(|| body.trim())
}

/// A Zuse export or a T3 Code palette, parsed leniently (comments and
/// trailing commas allowed); `None` hands the source to the VS Code compiler.
fn compile_own_format(
    value: &serde_json::Value,
    path: &Path,
    family_id: &str,
) -> Result<Option<SourceCompilation>> {
    match compile_native(value, path, family_id)? {
        Some(compilation) => Ok(Some(compilation)),
        None => crate::t3::compile(value, path, family_id),
    }
}

fn compile_native(
    value: &serde_json::Value,
    path: &Path,
    family_id: &str,
) -> Result<Option<SourceCompilation>> {
    if !value
        .get("variants")
        .is_some_and(serde_json::Value::is_array)
    {
        return Ok(None);
    }
    let mut family: ThemeFamily =
        serde_json::from_value(value.clone()).context("invalid Zuse theme JSON")?;
    if family.variants.is_empty() || family.variants.len() > MAX_LIBRARY_VARIANTS {
        bail!("a Zuse theme must contain between 1 and {MAX_LIBRARY_VARIANTS} variants");
    }
    if family.id != family_id {
        rekey_family(&mut family, family_id);
    }
    let errors = validation_errors(&family);
    if !errors.is_empty() {
        bail!("theme validation failed: {}", errors.join("; "));
    }
    Ok(Some(SourceCompilation {
        path: path.into(),
        source_kind: DetectedThemeSource::File,
        family,
        reports: BTreeMap::new(),
        failures: Vec::new(),
    }))
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

fn read_library_file(path: &Path) -> io::Result<String> {
    let size = fs::metadata(path)?.len();
    if size > MAX_LIBRARY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("custom theme library exceeds the {MAX_LIBRARY_BYTES}-byte limit"),
        ));
    }
    fs::read_to_string(path)
}

/// Replace `path` without relying on rename-over-existing semantics (which
/// Windows does not provide). The backup also lets startup recover if the
/// process stops between the two renames. Symlinks are not followed here: the
/// managed data-dir path itself is replaced.
fn replace_file_recoverably(path: &Path, contents: &[u8]) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    let backup = backup_path(path);
    let mut file = fs::File::create(&temporary)
        .with_context(|| format!("could not create {}", temporary.display()))?;
    file.write_all(contents)
        .with_context(|| format!("could not write {}", temporary.display()))?;
    file.sync_all()
        .with_context(|| format!("could not sync {}", temporary.display()))?;
    drop(file);

    if !path.exists() {
        fs::rename(&temporary, path)
            .with_context(|| format!("could not install {}", path.display()))?;
        return Ok(());
    }
    if backup.exists() {
        fs::remove_file(&backup)
            .with_context(|| format!("could not remove stale backup {}", backup.display()))?;
    }
    fs::rename(path, &backup).with_context(|| format!("could not back up {}", path.display()))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let restore = fs::rename(&backup, path);
        return match restore {
            Ok(()) => Err(error).with_context(|| format!("could not replace {}", path.display())),
            Err(restore_error) => Err(anyhow!(
                "could not replace {}; recovery copy remains at {} ({error}); restore failed: {restore_error}",
                path.display(),
                backup.display()
            )),
        };
    }
    // The committed primary is authoritative. A stale backup is harmless and
    // will be cleared before the next save; cleanup failure must not make the
    // caller believe an already-committed replacement failed.
    let _ = fs::remove_file(&backup);
    Ok(())
}

fn load_editable_family(path: &Path, expected_id: &str) -> Result<ThemeFamily> {
    let source = read_library_file(path)
        .with_context(|| format!("could not read editable theme {}", path.display()))?;
    let family: ThemeFamily = serde_json::from_str(&source)
        .with_context(|| format!("could not parse editable theme {}", path.display()))?;
    if family.id != expected_id {
        bail!(
            "editable theme family id must remain `{expected_id}` (found `{}`)",
            family.id
        );
    }
    let errors = validation_errors(&family);
    if !errors.is_empty() {
        bail!("editable theme validation failed: {}", errors.join("; "));
    }
    Ok(family)
}

fn validation_errors(family: &ThemeFamily) -> Vec<String> {
    ThemeRegistry {
        families: vec![family.clone()],
    }
    .validate()
    .into_iter()
    .filter(|issue| issue.is_blocking())
    .map(|issue| format!("{}: {}", issue.variant_id, issue.message))
    .collect()
}

fn unique_id<'a>(base: &str, existing: impl Iterator<Item = &'a str>) -> String {
    let existing = existing.collect::<HashSet<_>>();
    if !existing.contains(base) {
        return base.into();
    }
    (2..)
        .map(|suffix| format!("{base}-{suffix}"))
        .find(|candidate| !existing.contains(candidate.as_str()))
        .expect("an unused numeric suffix always exists")
}

fn rekey_family(family: &mut ThemeFamily, new_id: &str) {
    let old_id = family.id.clone();
    family.id = new_id.into();
    for variant in &mut family.variants {
        variant.family_id = new_id.into();
        variant.id = variant
            .id
            .strip_prefix(&old_id)
            .map(|suffix| format!("{new_id}{suffix}"))
            .unwrap_or_else(|| format!("{new_id}-{}", variant.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_fenced_t3_palette_with_an_opposite_variant() {
        let json = r##"{"version":1,"name":"Aurora","appearance":"dark","colors":{
          "canvas":"rgb(16 24 35)","accent":"oklch(76% 0.11 255)","text":"#e6eef9"
        },"variants":{"light":{"canvas":"#f4f7fb","text":"#27364a"}}}"##;
        let pasted = CustomThemeLibrary::compile_pasted(&format!("```json\n{json}\n```")).unwrap();
        assert_eq!(pasted.family.name, "Aurora");
        assert_eq!(pasted.family.variants.len(), 2);
        assert_eq!(
            pasted.family.variants[0].colors.background,
            "#101823".parse().unwrap()
        );
        assert_eq!(
            pasted.family.variants[1].appearance,
            crate::Appearance::Light
        );
        // The light variant sets no accent, so it inherits the theme's blue
        // rather than falling back to Zuse's olive.
        let zuse_light = ThemeRegistry::builtin().variant("zuse-light").unwrap();
        assert_ne!(
            pasted.family.variants[1].accent.primary,
            zuse_light.accent.primary
        );
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("aurora.json");
        fs::write(&path, json).unwrap();
        let local = CustomThemeLibrary::compile(&path, "custom-aurora", "Aurora").unwrap();
        assert_eq!(local.path, path.canonicalize().unwrap());
        assert_eq!(
            local.family.variants[0].accent,
            pasted.family.variants[0].accent
        );
    }

    #[test]
    fn community_packages_follow_includes_and_ignore_unrelated_files() {
        let options = || CompileOptions {
            family_id: "custom-shared".into(),
            family_name: "Shared".into(),
            source_url: "https://open-vsx.org/extension/sample/shared".into(),
            revision: "1.0.0".into(),
            license: "MIT".into(),
        };
        let package = |files: &[(&str, &[u8])]| {
            let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, contents) in files {
                archive
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                archive.write_all(contents).unwrap();
            }
            archive.finish().unwrap().into_inner()
        };
        let huge = vec![b' '; MAX_THEME_JSON_BYTES as usize + 1];
        // A theme including a file from a sibling folder.
        let included = package(&[
            (
                "extension/package.json",
                br#"{"contributes":{"themes":[{"label":"Shared Dark","uiTheme":"vs-dark","path":"./themes/dark.json"}]}}"#,
            ),
            (
                "extension/themes/dark.json",
                br##"{"include":"../shared/base.json","colors":{"editor.foreground":"#e2e2f0"}}"##,
            ),
            (
                "extension/shared/base.json",
                br##"{"colors":{"editor.background":"#202030"}}"##,
            ),
            ("extension/i18n/huge.json", &huge),
        ]);
        let compiled = CustomThemeLibrary::compile_community_package(&included, options()).unwrap();
        assert_eq!(
            compiled.family.variants[0].colors.background,
            "#202030".parse().unwrap()
        );
        // A root-level theme does not admit every other folder.
        let root_level = package(&[
            (
                "extension/package.json",
                br#"{"contributes":{"themes":[{"label":"Root Dark","uiTheme":"vs-dark","path":"dark.json"}]}}"#,
            ),
            (
                "extension/dark.json",
                br##"{"colors":{"editor.background":"#101010","editor.foreground":"#eeeeee"}}"##,
            ),
            ("extension/i18n/huge.json", &huge),
        ]);
        assert!(CustomThemeLibrary::compile_community_package(&root_level, options()).is_ok());
        // Includes cannot climb out of the extension.
        assert_eq!(
            package_relative(Path::new("themes"), "../../etc/passwd"),
            None
        );
    }

    #[test]
    fn pasted_json_accepts_long_and_tilde_code_fences() {
        let json =
            r##"{"version":1,"name":"Fenced","appearance":"dark","colors":{"canvas":"#101823"}}"##;
        for fenced in [
            format!("````json\n{json}\n````"),
            format!("~~~\n{json}\n~~~~"),
            format!("```\n{json}\n```  "),
        ] {
            let compiled = CustomThemeLibrary::compile_pasted(&fenced).unwrap();
            assert_eq!(compiled.family.name, "Fenced");
        }
        // A closing fence shorter than the opener does not close it.
        assert!(strip_code_fence(&format!("````\n{json}\n```")).is_none());
    }

    #[test]
    fn t3_terminal_selection_blends_over_the_terminal_background() {
        let json = r##"{"version":1,"name":"Term","appearance":"light","colors":{
          "canvas":"#ffffff","terminalBackground":"#000000",
          "terminalForeground":"#ffffff","terminalSelection":"#ffffff80"}}"##;
        let compiled = CustomThemeLibrary::compile_pasted(json).unwrap();
        let terminal = &compiled.family.variants[0].terminal;
        // Half-white over black is mid-grey, not the canvas-blended white
        // that would hide white terminal text.
        let selection = terminal.selection;
        assert!((100..=156).contains(&selection.r), "{selection:?}");
    }

    #[test]
    fn vscode_themes_with_version_metadata_are_not_t3() {
        let json = r##"{"name":"Meta","version":"1.0.0","appearance":"dark",
          "colors":{"editor.background":"#161625","editor.foreground":"#e2e2f0"}}"##;
        let compiled = CustomThemeLibrary::compile_pasted(json).unwrap();
        assert_eq!(
            compiled.family.variants[0].colors.background,
            "#161625".parse().unwrap()
        );
    }

    #[test]
    fn native_export_with_comments_is_still_a_zuse_theme() {
        let original = ThemeRegistry::builtin()
            .families
            .iter()
            .find(|family| family.id == "nord")
            .unwrap();
        let json = serde_json::to_string_pretty(original).unwrap();
        let commented = format!(
            "// tweaked by hand\n{}",
            json.replacen('{', "{ /* hi */", 1)
        );
        let compilation = CustomThemeLibrary::compile_pasted(&commented).unwrap();
        assert_eq!(compilation.family.name, original.name);
        assert_eq!(
            compilation.family.variants[0].colors,
            original.variants[0].colors
        );
    }

    #[test]
    fn native_copy_can_be_pasted_installed_and_exported_again() {
        let original = ThemeRegistry::builtin()
            .families
            .iter()
            .find(|family| family.id == "observatory")
            .unwrap();
        let json = serde_json::to_string(original).unwrap();
        let compilation = CustomThemeLibrary::compile_pasted(&json).unwrap();
        assert!(compilation.path.as_os_str().is_empty());
        assert_eq!(compilation.family.name, original.name);
        assert_eq!(compilation.family.variants.len(), 2);

        let mut library = CustomThemeLibrary::default();
        assert!(
            library
                .install(compilation.clone(), &[], InstallMode::Link)
                .is_err()
        );
        let id = library
            .install(compilation, &[], InstallMode::Snapshot)
            .unwrap();
        let installed = library.entry(&id).unwrap();
        assert!(installed.source.path().is_none());
        assert_eq!(
            installed.family.variants[0].colors,
            original.variants[0].colors
        );
        assert_eq!(
            installed.family.variants[1].colors,
            original.variants[1].colors
        );
    }

    #[test]
    fn community_package_imports_declared_theme_data() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        archive
            .start_file("extension/package.json", options)
            .unwrap();
        archive.write_all(br#"{"displayName":"Community Theme","main":"./index.js","contributes":{"themes":[{"label":"Community Dark","uiTheme":"vs-dark","path":"themes/dark.json"}]}}"#).unwrap();
        archive
            .start_file("extension/themes/dark.json", options)
            .unwrap();
        archive
            .write_all(
                br##"{"colors":{"editor.background":"#161625","editor.foreground":"#e2e2f0"}}"##,
            )
            .unwrap();
        // Bundled dependencies and data outside the theme folders must not
        // count against the extraction limits.
        archive
            .start_file("extension/node_modules/big/data.json", options)
            .unwrap();
        archive
            .write_all(&vec![b' '; MAX_THEME_JSON_BYTES as usize + 1])
            .unwrap();
        archive
            .start_file("extension/i18n/huge.json", options)
            .unwrap();
        archive
            .write_all(&vec![b' '; MAX_THEME_JSON_BYTES as usize + 1])
            .unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        let compiled = CustomThemeLibrary::compile_community_package(
            &bytes,
            CompileOptions {
                family_id: "custom-community-theme".into(),
                family_name: "Community Theme".into(),
                source_url: "https://open-vsx.org/extension/sample/theme".into(),
                revision: "1.0.0".into(),
                license: "MIT".into(),
            },
        )
        .unwrap();
        assert!(compiled.path.as_os_str().is_empty());
        let variant = &compiled.family.variants[0];
        assert_eq!(variant.name, "Community Dark");
        assert_eq!(variant.colors.background, "#161625".parse().unwrap());
        assert_eq!(variant.source.revision, "1.0.0");
    }

    fn package(dir: &Path) {
        fs::create_dir_all(dir.join("themes")).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{
              "displayName": "Test Family",
              "contributes": { "themes": [
                { "label": "Test Light", "uiTheme": "vs", "path": "./themes/light.json" },
                { "label": "Test Dark", "uiTheme": "vs-dark", "path": "./themes/dark.json" }
              ] }
            }"#,
        )
        .unwrap();
        fs::write(
            dir.join("themes/light.json"),
            r##"{"colors":{"editor.background":"#ffffff","foreground":"#222222","focusBorder":"#0066cc"}}"##,
        )
        .unwrap();
        fs::write(
            dir.join("themes/dark.json"),
            r##"{"colors":{"editor.background":"#111111","foreground":"#eeeeee","focusBorder":"#66aaff"}}"##,
        )
        .unwrap();
    }

    #[test]
    fn linked_reload_keeps_last_known_good_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        package(dir.path());
        let compiled =
            CustomThemeLibrary::compile(dir.path(), "test-family", "Test Family").unwrap();
        let mut library = CustomThemeLibrary::default();
        let id = library.install(compiled, &[], InstallMode::Link).unwrap();
        let previous = library.entry(&id).unwrap().family.clone();
        fs::write(dir.path().join("themes/dark.json"), "not json").unwrap();

        assert!(library.reload(&id).is_err());
        let entry = library.entry(&id).unwrap();
        assert_eq!(entry.family, previous);
        assert!(matches!(entry.status, CustomThemeStatus::Warning { .. }));
    }

    #[test]
    fn linked_reload_ignores_failures_in_unselected_sibling_variants() {
        let dir = tempfile::tempdir().unwrap();
        package(dir.path());
        let compiled =
            CustomThemeLibrary::compile(dir.path(), "test-family", "Test Family").unwrap();
        let selected = vec!["test-family-test-light".to_string()];
        let mut library = CustomThemeLibrary::default();
        let id = library
            .install(compiled, &selected, InstallMode::Link)
            .unwrap();
        fs::write(dir.path().join("themes/dark.json"), "not json").unwrap();
        fs::write(
            dir.path().join("themes/light.json"),
            r##"{"colors":{"editor.background":"#fafafa","foreground":"#202020","focusBorder":"#0066cc"}}"##,
        )
        .unwrap();

        library.reload(&id).unwrap();
        let entry = library.entry(&id).unwrap();
        assert_eq!(entry.family.variants.len(), 1);
        assert_eq!(
            entry.family.variants[0].colors.background,
            "#fafafa".parse().unwrap()
        );
        assert_eq!(entry.status, CustomThemeStatus::Ready);
    }

    #[test]
    fn linked_reload_hardens_low_contrast_palette() {
        let dir = tempfile::tempdir().unwrap();
        package(dir.path());
        let compiled =
            CustomThemeLibrary::compile(dir.path(), "test-family", "Test Family").unwrap();
        let mut library = CustomThemeLibrary::default();
        let id = library.install(compiled, &[], InstallMode::Link).unwrap();
        fs::write(
            dir.path().join("themes/dark.json"),
            r##"{"colors":{"editor.background":"#111111","foreground":"#111111","focusBorder":"#66aaff"}}"##,
        )
        .unwrap();

        library.reload(&id).unwrap();
        let entry = library.entry(&id).unwrap();
        let variant = &entry.family.variants[0];
        assert!(variant.colors.text.contrast(variant.colors.background) >= 4.5);
        assert!(
            entry
                .reports
                .values()
                .any(|report| !report.adjustments.is_empty())
        );
        assert_eq!(entry.status, CustomThemeStatus::Ready);
    }

    #[test]
    fn snapshot_round_trips_and_duplicate_gets_independent_ids() {
        let source = tempfile::tempdir().unwrap();
        package(source.path());
        let compiled =
            CustomThemeLibrary::compile(source.path(), "test-family", "Test Family").unwrap();
        let mut library = CustomThemeLibrary::default();
        let id = library
            .install(compiled, &[], InstallMode::Snapshot)
            .unwrap();
        let copy = library.duplicate_as_snapshot(&id).unwrap();
        assert_ne!(id, copy);
        assert!(
            library
                .entry(&copy)
                .unwrap()
                .family
                .variants
                .iter()
                .all(|variant| variant.family_id == copy)
        );

        let data = tempfile::tempdir().unwrap();
        library.save(data.path()).unwrap();
        library.save(data.path()).unwrap();
        assert_eq!(CustomThemeLibrary::load(data.path()).unwrap(), library);
    }

    #[test]
    fn load_recovers_from_backup_left_between_replacement_renames() {
        let data = tempfile::tempdir().unwrap();
        let library = CustomThemeLibrary::default();
        library.save(data.path()).unwrap();
        let path = CustomThemeLibrary::path(data.path());
        fs::rename(&path, backup_path(&path)).unwrap();

        assert_eq!(CustomThemeLibrary::load(data.path()).unwrap(), library);
    }

    #[test]
    fn rejects_libraries_above_entry_limit_before_serializing_or_cloning() {
        let source = tempfile::tempdir().unwrap();
        package(source.path());
        let compiled =
            CustomThemeLibrary::compile(source.path(), "test-family", "Test Family").unwrap();
        let mut library = CustomThemeLibrary::default();
        let id = library
            .install(compiled, &[], InstallMode::Snapshot)
            .unwrap();
        let template = library.entry(&id).unwrap().clone();
        while library.entries.len() <= MAX_LIBRARY_ENTRIES {
            library.entries.push(template.clone());
        }

        let data = tempfile::tempdir().unwrap();
        let error = library.save(data.path()).unwrap_err();
        assert!(error.to_string().contains("limited to 256 entries"));
        assert!(!CustomThemeLibrary::path(data.path()).exists());
    }

    #[test]
    fn contrast_findings_do_not_block_install() {
        let source = tempfile::tempdir().unwrap();
        package(source.path());
        let mut compiled =
            CustomThemeLibrary::compile(source.path(), "test-family", "Test Family").unwrap();
        let variant = &mut compiled.family.variants[0];
        variant.colors.text = variant.colors.background;

        CustomThemeLibrary::default()
            .install(compiled, &[], InstallMode::Snapshot)
            .unwrap();
    }

    #[test]
    fn install_rejects_structurally_invalid_palettes() {
        let source = tempfile::tempdir().unwrap();
        package(source.path());
        let mut compiled =
            CustomThemeLibrary::compile(source.path(), "test-family", "Test Family").unwrap();
        compiled.family.variants[0].source.asset_hash.clear();

        let error = CustomThemeLibrary::default()
            .install(compiled, &[], InstallMode::Snapshot)
            .unwrap_err();
        assert!(error.to_string().contains("theme validation failed"));
    }

    #[test]
    fn editable_duplicate_reloads_and_keeps_last_known_good_on_failure() {
        let source = tempfile::tempdir().unwrap();
        package(source.path());
        let compiled =
            CustomThemeLibrary::compile(source.path(), "test-family", "Test Family").unwrap();
        let mut library = CustomThemeLibrary::default();
        let id = library
            .install(compiled, &[], InstallMode::Snapshot)
            .unwrap();
        let data = tempfile::tempdir().unwrap();
        let copy = library.duplicate_as_editable(&id, data.path()).unwrap();
        let path = match &library.entry(&copy).unwrap().source {
            CustomThemeSource::EditableFile { path } => path.clone(),
            source => panic!("expected editable source, found {source:?}"),
        };

        let mut edited = library.entry(&copy).unwrap().family.clone();
        edited.name = "Edited Copy".into();
        fs::write(&path, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
        library.reload(&copy).unwrap();
        assert_eq!(library.entry(&copy).unwrap().name, "Edited Copy");

        let last_known_good = library.entry(&copy).unwrap().family.clone();
        fs::write(&path, "not json").unwrap();
        assert!(library.reload(&copy).is_err());
        let entry = library.entry(&copy).unwrap();
        assert_eq!(entry.family, last_known_good);
        assert!(matches!(entry.status, CustomThemeStatus::Warning { .. }));
    }
}

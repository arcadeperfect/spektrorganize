//! Read side for the library UI: paged, filtered asset listings; facet counts; asset detail.

use super::keywords::{self, AssetKeyword};
use super::Catalog;
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, params_from_iter};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// All set fields must match. Text is split into words; each word must match a file name or
/// path, a keyword, the camera or the lens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub roots: Vec<i64>,
    /// Inclusive, `YYYY-MM-DD` (or any prefix of an ISO timestamp).
    pub date_from: Option<String>,
    /// Inclusive, `YYYY-MM-DD`.
    pub date_to: Option<String>,
    /// Any of these cameras; `""` means "no camera recorded".
    pub cameras: Vec<String>,
    /// Every one of these keywords (case-insensitive, any source).
    pub keywords: Vec<String>,
    pub has_render: Option<bool>,
    /// Any of: raw, image, video.
    pub kinds: Vec<String>,
    pub text: Option<String>,
    pub min_rating: Option<i64>,
    /// Some(true): has AI labels; Some(false): has none yet.
    pub ai_labelled: Option<bool>,
    /// Only assets whose primary file is missing (or, with Some(false), only present ones).
    pub missing: Option<bool>,
    /// Assets without a capture date only.
    pub undated: bool,
    /// Any of: "select", "reject", "none" (no flag).
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    #[default]
    CapturedDesc,
    CapturedAsc,
    AddedDesc,
    Name,
}

impl Sort {
    fn sql(self) -> &'static str {
        match self {
            Sort::CapturedDesc => "a.captured_at IS NULL, a.captured_at DESC, a.id DESC",
            Sort::CapturedAsc => "a.captured_at IS NULL, a.captured_at ASC, a.id ASC",
            Sort::AddedDesc => "a.id DESC",
            Sort::Name => "f.name COLLATE NOCASE, a.id",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetSummary {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub rating: i64,
    /// "select", "reject", or empty.
    pub flag: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// Number of files (RAW + JPEG = 2).
    pub files: i64,
    /// A camera JPEG is linked to the RAW.
    pub has_jpeg: bool,
    pub renders: i64,
    pub ai_labelled: bool,
    /// 256 px thumbnail, when generated.
    pub thumb: Option<String>,
    pub missing: bool,
    pub online: bool,
    pub root_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub total: i64,
    pub offset: i64,
    pub items: Vec<AssetSummary>,
}

const FROM: &str = " FROM assets a LEFT JOIN files f ON f.id = a.primary_file_id LEFT JOIN roots r ON r.id = f.root_id ";

fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('%');
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn next_day(date: &str) -> String {
    // "YYYY-MM-DD" -> the following day, so a whole day is included with `<`.
    chrono::NaiveDate::parse_from_str(&date[..date.len().min(10)], "%Y-%m-%d")
        .ok()
        .and_then(|d| d.succ_opt())
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| format!("{date}\u{ffff}"))
}

/// WHERE clause (starting with " WHERE 1") and its parameters.
fn where_clause(f: &Filter) -> (String, Vec<Value>) {
    let mut sql = String::from(" WHERE 1");
    let mut p: Vec<Value> = Vec::new();
    let placeholders = |n: usize| vec!["?"; n].join(", ");
    if !f.roots.is_empty() {
        sql += &format!(" AND f.root_id IN ({})", placeholders(f.roots.len()));
        p.extend(f.roots.iter().map(|r| Value::Integer(*r)));
    }
    if let Some(d) = f.date_from.as_deref().filter(|d| !d.is_empty()) {
        sql += " AND a.captured_at >= ?";
        p.push(Value::Text(d.to_string()));
    }
    if let Some(d) = f.date_to.as_deref().filter(|d| !d.is_empty()) {
        sql += " AND a.captured_at < ?";
        p.push(Value::Text(next_day(d)));
    }
    if f.undated {
        sql += " AND a.captured_at IS NULL";
    }
    if !f.cameras.is_empty() {
        let named: Vec<&String> = f.cameras.iter().filter(|c| !c.is_empty()).collect();
        let unknown = f.cameras.iter().any(|c| c.is_empty());
        let mut parts = Vec::new();
        if !named.is_empty() {
            parts.push(format!("a.camera IN ({})", placeholders(named.len())));
            p.extend(named.into_iter().map(|c| Value::Text(c.clone())));
        }
        if unknown {
            parts.push("a.camera IS NULL".to_string());
        }
        sql += &format!(" AND ({})", parts.join(" OR "));
    }
    for k in &f.keywords {
        sql += " AND EXISTS (SELECT 1 FROM asset_keywords ak JOIN keywords k ON k.id = ak.keyword_id WHERE ak.asset_id = a.id AND k.name = ?)";
        p.push(Value::Text(k.clone()));
    }
    match f.has_render {
        Some(true) => sql += " AND EXISTS (SELECT 1 FROM renders rr WHERE rr.asset_id = a.id)",
        Some(false) => sql += " AND NOT EXISTS (SELECT 1 FROM renders rr WHERE rr.asset_id = a.id)",
        None => {}
    }
    if let Some(b) = f.ai_labelled {
        sql += if b { " AND EXISTS" } else { " AND NOT EXISTS" };
        sql += " (SELECT 1 FROM asset_keywords ak WHERE ak.asset_id = a.id AND ak.source LIKE 'ai:%')";
    }
    match f.missing {
        Some(true) => sql += " AND f.missing = 1",
        Some(false) => sql += " AND f.missing = 0",
        None => {}
    }
    if !f.kinds.is_empty() {
        sql += &format!(" AND a.kind IN ({})", placeholders(f.kinds.len()));
        p.extend(f.kinds.iter().map(|k| Value::Text(k.clone())));
    }
    if !f.flags.is_empty() {
        let parts: Vec<&str> = f
            .flags
            .iter()
            .map(|x| match x.as_str() {
                "select" => "a.flag = 'select'",
                "reject" => "a.flag = 'reject'",
                _ => "a.flag IS NULL",
            })
            .collect();
        sql += &format!(" AND ({})", parts.join(" OR "));
    }
    if let Some(r) = f.min_rating.filter(|r| *r > 0) {
        sql += " AND a.rating >= ?";
        p.push(Value::Integer(r));
    }
    if let Some(text) = &f.text {
        for word in text.split_whitespace() {
            let pat = like_escape(word);
            sql += " AND (EXISTS (SELECT 1 FROM asset_files af JOIN files ff ON ff.id = af.file_id WHERE af.asset_id = a.id AND ff.rel LIKE ? ESCAPE '\\')
                     OR EXISTS (SELECT 1 FROM asset_keywords ak JOIN keywords k ON k.id = ak.keyword_id WHERE ak.asset_id = a.id AND k.name LIKE ? ESCAPE '\\')
                     OR a.camera LIKE ? ESCAPE '\\' OR a.lens LIKE ? ESCAPE '\\')";
            for _ in 0..4 {
                p.push(Value::Text(pat.clone()));
            }
        }
    }
    (sql, p)
}

impl Catalog {
    pub fn count(&self, f: &Filter) -> anyhow::Result<i64> {
        let (w, p) = where_clause(f);
        Ok(self.conn().query_row(&format!("SELECT COUNT(*){FROM}{w}"), params_from_iter(p), |r| r.get(0))?)
    }

    /// One page of assets matching `f`, plus the total count.
    pub fn list(&self, f: &Filter, sort: Sort, offset: i64, limit: i64) -> anyhow::Result<Page> {
        let total = self.count(f)?;
        let (w, mut p) = where_clause(f);
        let sql = format!(
            "SELECT a.id, a.kind, COALESCE(f.name, ''), a.captured_at, a.camera, a.rating, COALESCE(a.flag, ''), a.width, a.height,
                    (SELECT COUNT(*) FROM asset_files af WHERE af.asset_id = a.id),
                    EXISTS (SELECT 1 FROM asset_files af WHERE af.asset_id = a.id AND af.role = 'jpeg'),
                    (SELECT COUNT(*) FROM renders rr WHERE rr.asset_id = a.id),
                    EXISTS (SELECT 1 FROM asset_keywords ak WHERE ak.asset_id = a.id AND ak.source LIKE 'ai:%'),
                    (SELECT t.path FROM thumbnails t WHERE t.asset_id = a.id AND t.size = 256),
                    COALESCE(f.missing, 1), COALESCE(r.online, 0), f.root_id
             {FROM}{w} ORDER BY {} LIMIT ? OFFSET ?",
            sort.sql()
        );
        p.push(Value::Integer(limit.clamp(1, 5000)));
        p.push(Value::Integer(offset.max(0)));
        let mut stmt = self.conn().prepare(&sql)?;
        let items = stmt
            .query_map(params_from_iter(p), |r| {
                Ok(AssetSummary {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    name: r.get(2)?,
                    captured_at: r.get(3)?,
                    camera: r.get(4)?,
                    rating: r.get(5)?,
                    flag: r.get(6)?,
                    width: r.get(7)?,
                    height: r.get(8)?,
                    files: r.get(9)?,
                    has_jpeg: r.get(10)?,
                    renders: r.get(11)?,
                    ai_labelled: r.get(12)?,
                    thumb: r.get(13)?,
                    missing: r.get::<_, i64>(14)? != 0,
                    online: r.get::<_, i64>(15)? != 0,
                    root_id: r.get(16)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(Page { total, offset, items })
    }

    /// Every id matching `f`, in `sort` order (select-all, printing a whole view).
    pub fn ids(&self, f: &Filter, sort: Sort) -> anyhow::Result<Vec<i64>> {
        let (w, p) = where_clause(f);
        let mut stmt = self.conn().prepare(&format!("SELECT a.id{FROM}{w} ORDER BY {}", sort.sql()))?;
        let ids = stmt.query_map(params_from_iter(p), |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(ids)
    }

    pub fn facets(&self) -> anyhow::Result<Facets> {
        let c = self.conn();
        let total: i64 = c.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))?;
        let undated: i64 = c.query_row("SELECT COUNT(*) FROM assets WHERE captured_at IS NULL", [], |r| r.get(0))?;
        let selected: i64 = c.query_row("SELECT COUNT(*) FROM assets WHERE flag = 'select'", [], |r| r.get(0))?;
        let rejected: i64 = c.query_row("SELECT COUNT(*) FROM assets WHERE flag = 'reject'", [], |r| r.get(0))?;
        let rated: i64 = c.query_row("SELECT COUNT(*) FROM assets WHERE rating > 0", [], |r| r.get(0))?;
        let rendered: i64 = c.query_row("SELECT COUNT(DISTINCT asset_id) FROM renders", [], |r| r.get(0))?;
        let ai_labelled: i64 = c.query_row("SELECT COUNT(DISTINCT asset_id) FROM asset_keywords WHERE source LIKE 'ai:%'", [], |r| r.get(0))?;
        let missing: i64 = c.query_row("SELECT COUNT(*) FROM assets a JOIN files f ON f.id = a.primary_file_id WHERE f.missing = 1", [], |r| r.get(0))?;
        let cameras = {
            let mut stmt = c.prepare("SELECT COALESCE(camera, ''), COUNT(*) FROM assets GROUP BY camera ORDER BY COUNT(*) DESC, camera")?;
            stmt.query_map([], |r| Ok(Count { key: r.get(0)?, count: r.get(1)? }))?.collect::<Result<_, _>>()?
        };
        let kinds = {
            let mut stmt = c.prepare("SELECT kind, COUNT(*) FROM assets GROUP BY kind ORDER BY COUNT(*) DESC")?;
            stmt.query_map([], |r| Ok(Count { key: r.get(0)?, count: r.get(1)? }))?.collect::<Result<_, _>>()?
        };
        let months = {
            let mut stmt = c.prepare(
                "SELECT substr(captured_at, 1, 7), COUNT(*) FROM assets WHERE captured_at IS NOT NULL GROUP BY 1 ORDER BY 1 DESC",
            )?;
            stmt.query_map([], |r| Ok(Count { key: r.get(0)?, count: r.get(1)? }))?.collect::<Result<_, _>>()?
        };
        let keywords = {
            let mut stmt = c.prepare(
                "SELECT k.name, COUNT(*), SUM(ak.source <> 'user') FROM keywords k JOIN asset_keywords ak ON ak.keyword_id = k.id
                 GROUP BY k.id ORDER BY COUNT(*) DESC, k.name COLLATE NOCASE",
            )?;
            stmt.query_map([], |r| Ok(KeywordCount { name: r.get(0)?, count: r.get(1)?, ai: r.get(2)? }))?.collect::<Result<_, _>>()?
        };
        Ok(Facets { total, undated, selected, rejected, rated, rendered, ai_labelled, missing, cameras, kinds, months, keywords, roots: self.roots()? })
    }

    pub fn detail(&self, id: i64) -> anyhow::Result<AssetDetail> {
        let c = self.conn();
        let row = c
            .query_row(
                "SELECT a.id, a.kind, a.captured_at, a.make, a.model, a.camera, a.lens, a.iso, a.width, a.height, a.orientation,
                        a.meta_source, a.rating, COALESCE(a.flag, ''), a.added_at, a.primary_file_id, i.source_label, i.created_at
                 FROM assets a LEFT JOIN imports i ON i.id = a.import_id WHERE a.id = ?1",
                [id],
                |r| {
                    Ok(AssetDetail {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        captured_at: r.get(2)?,
                        make: r.get(3)?,
                        model: r.get(4)?,
                        camera: r.get(5)?,
                        lens: r.get(6)?,
                        iso: r.get(7)?,
                        width: r.get(8)?,
                        height: r.get(9)?,
                        orientation: r.get(10)?,
                        meta_source: r.get(11)?,
                        rating: r.get(12)?,
                        flag: r.get(13)?,
                        added_at: r.get(14)?,
                        primary_file_id: r.get(15)?,
                        import_source: r.get(16)?,
                        imported_at: r.get(17)?,
                        files: Vec::new(),
                        keywords: Vec::new(),
                        renders: Vec::new(),
                        thumb: None,
                        preview: None,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("no asset #{id}"))?;
        let mut d = row;
        d.files = {
            let mut stmt = c.prepare(
                "SELECT f.id, af.role, f.kind, f.name, f.rel, r.path, r.online, f.size, f.blake3, f.missing, f.root_id
                 FROM asset_files af JOIN files f ON f.id = af.file_id JOIN roots r ON r.id = f.root_id
                 WHERE af.asset_id = ?1 ORDER BY f.id = ?2 DESC, f.name",
            )?;
            stmt.query_map(rusqlite::params![id, d.primary_file_id], |r| {
                let root: String = r.get(5)?;
                let rel: String = r.get(4)?;
                Ok(FileInfo {
                    id: r.get(0)?,
                    role: r.get(1)?,
                    kind: r.get(2)?,
                    name: r.get(3)?,
                    path: PathBuf::from(root).join(&rel),
                    rel,
                    online: r.get::<_, i64>(6)? != 0,
                    size: r.get(7)?,
                    blake3: r.get(8)?,
                    missing: r.get::<_, i64>(9)? != 0,
                    root_id: r.get(10)?,
                })
            })?
            .collect::<Result<_, _>>()?
        };
        d.keywords = keywords::of_asset(c, id)?;
        d.renders = {
            let mut stmt = c.prepare(
                "SELECT rr.id, rr.kind, rr.preset_name, rr.preset_hash, r.path, rr.rel, rr.created_at
                 FROM renders rr JOIN roots r ON r.id = rr.root_id WHERE rr.asset_id = ?1 ORDER BY rr.created_at DESC",
            )?;
            stmt.query_map([id], |r| {
                let path = PathBuf::from(r.get::<_, String>(4)?).join(r.get::<_, String>(5)?);
                Ok(RenderInfo {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    preset_name: r.get(2)?,
                    preset_hash: r.get(3)?,
                    exists: path.exists(),
                    path,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?
        };
        let thumb = |size: i64| -> anyhow::Result<Option<String>> {
            Ok(c
                .query_row("SELECT path FROM thumbnails WHERE asset_id = ?1 AND size = ?2", rusqlite::params![id, size], |r| r.get::<_, Option<String>>(0))
                .optional()?
                .flatten())
        };
        d.thumb = thumb(256)?;
        d.preview = thumb(1024)?;
        Ok(d)
    }
}

/// What the AI labeller is told about an asset besides its thumbnail.
#[derive(Debug, Clone, Serialize)]
pub struct LabelInfo {
    pub id: i64,
    pub kind: String,
    pub name: String,
    /// Folder of the primary file inside its root ("" at the top).
    pub folder: String,
    /// Root label (folder name of the archive or added folder).
    pub root: String,
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// The user's own keywords (never replaced by labelling).
    pub keywords: Vec<String>,
    pub ai_keywords: Vec<String>,
}

impl Catalog {
    pub fn label_info(&self, ids: &[i64]) -> anyhow::Result<Vec<LabelInfo>> {
        let mut out = Vec::with_capacity(ids.len());
        let mut stmt = self.conn().prepare_cached(
            "SELECT a.id, a.kind, COALESCE(f.name, ''), COALESCE(f.rel, ''), COALESCE(r.label, ''), a.captured_at, a.camera, a.lens
             FROM assets a LEFT JOIN files f ON f.id = a.primary_file_id LEFT JOIN roots r ON r.id = f.root_id WHERE a.id = ?1",
        )?;
        for id in ids {
            let Some(mut info) = stmt
                .query_row([id], |r| {
                    let rel: String = r.get(3)?;
                    Ok(LabelInfo {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        name: r.get(2)?,
                        folder: rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default(),
                        root: r.get(4)?,
                        captured_at: r.get(5)?,
                        camera: r.get(6)?,
                        lens: r.get(7)?,
                        keywords: Vec::new(),
                        ai_keywords: Vec::new(),
                    })
                })
                .optional()?
            else {
                continue;
            };
            for k in keywords::of_asset(self.conn(), *id)? {
                if k.source == keywords::USER {
                    info.keywords.push(k.name);
                } else {
                    info.ai_keywords.push(k.name);
                }
            }
            out.push(info);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Count {
    pub key: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeywordCount {
    pub name: String,
    pub count: i64,
    /// How many of those are AI labels.
    pub ai: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Facets {
    pub total: i64,
    pub undated: i64,
    pub selected: i64,
    pub rejected: i64,
    pub rated: i64,
    pub rendered: i64,
    pub ai_labelled: i64,
    pub missing: i64,
    /// `key` "" = no camera recorded.
    pub cameras: Vec<Count>,
    pub kinds: Vec<Count>,
    /// `key` = "YYYY-MM", newest first.
    pub months: Vec<Count>,
    pub keywords: Vec<KeywordCount>,
    pub roots: Vec<super::Root>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub id: i64,
    pub role: String,
    pub kind: String,
    pub name: String,
    pub rel: String,
    pub path: PathBuf,
    pub root_id: i64,
    pub online: bool,
    pub size: i64,
    pub blake3: Option<String>,
    pub missing: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderInfo {
    pub id: i64,
    pub kind: String,
    pub preset_name: Option<String>,
    pub preset_hash: String,
    pub path: PathBuf,
    pub created_at: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetDetail {
    pub id: i64,
    pub kind: String,
    pub captured_at: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub orientation: Option<i64>,
    pub meta_source: Option<String>,
    pub rating: i64,
    /// "select", "reject" or "" for neither.
    pub flag: String,
    pub added_at: String,
    pub primary_file_id: Option<i64>,
    pub import_source: Option<String>,
    pub imported_at: Option<String>,
    pub files: Vec<FileInfo>,
    pub keywords: Vec<AssetKeyword>,
    pub renders: Vec<RenderInfo>,
    /// 256 px thumbnail, when generated.
    pub thumb: Option<String>,
    /// 1024 px preview, when generated.
    pub preview: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::index::add_folder;
    use crate::catalog::testutil::write;
    use crate::job::Cancel;

    /// Four assets with hand-set metadata.
    fn fixture() -> (tempfile::TempDir, Catalog, std::collections::HashMap<String, i64>) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        write(&root, "2024/beach/DSCF0001.RAF", 10, 1_700_000_000);
        write(&root, "2024/beach/DSCF0001.JPG", 10, 1_700_000_000);
        write(&root, "2024/city/DSC00002.ARW", 10, 1_700_000_000);
        write(&root, "2025/misc/IMG_0003.JPG", 10, 1_700_000_000);
        write(&root, "2025/misc/clip.mp4", 10, 1_700_000_000);
        let mut cat = Catalog::open_in_memory().unwrap();
        add_folder(&mut cat, &root, &mut |_| {}, &Cancel::new()).unwrap();
        let mut ids = std::collections::HashMap::new();
        for (name, captured, camera) in [
            ("DSCF0001.RAF", Some("2024-10-04T13:55:10"), Some("X-T10")),
            ("DSC00002.ARW", Some("2024-10-05T09:00:00"), Some("ILCE-7M3")),
            ("IMG_0003.JPG", Some("2025-01-02T12:00:00"), None),
            ("clip.mp4", None, None),
        ] {
            let id: i64 = cat
                .conn()
                .query_row("SELECT a.id FROM assets a JOIN files f ON f.id = a.primary_file_id WHERE f.name = ?1", [name], |r| r.get(0))
                .unwrap();
            cat.conn().execute("UPDATE assets SET captured_at = ?2, camera = ?3 WHERE id = ?1", rusqlite::params![id, captured, camera]).unwrap();
            ids.insert(name.to_string(), id);
        }
        (tmp, cat, ids)
    }

    fn names(cat: &Catalog, f: &Filter) -> Vec<String> {
        cat.list(f, Sort::CapturedDesc, 0, 100).unwrap().items.into_iter().map(|a| a.name).collect()
    }

    #[test]
    fn lists_filters_and_pages() {
        let (_tmp, cat, ids) = fixture();
        // Newest first, undated last.
        assert_eq!(names(&cat, &Filter::default()), vec!["IMG_0003.JPG", "DSC00002.ARW", "DSCF0001.RAF", "clip.mp4"]);
        let page = cat.list(&Filter::default(), Sort::CapturedDesc, 1, 2).unwrap();
        assert_eq!(page.total, 4);
        assert_eq!(page.items.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["DSC00002.ARW", "DSCF0001.RAF"]);
        let pair = cat.list(&Filter::default(), Sort::CapturedDesc, 0, 10).unwrap().items.into_iter().find(|a| a.name == "DSCF0001.RAF").unwrap();
        assert!(pair.has_jpeg);
        assert_eq!(pair.files, 2);

        // Date range is inclusive of whole days.
        let f = Filter { date_from: Some("2024-10-04".into()), date_to: Some("2024-10-04".into()), ..Default::default() };
        assert_eq!(names(&cat, &f), vec!["DSCF0001.RAF"]);
        let f = Filter { date_from: Some("2024-10-05".into()), ..Default::default() };
        assert_eq!(names(&cat, &f), vec!["IMG_0003.JPG", "DSC00002.ARW"]);
        assert_eq!(names(&cat, &Filter { undated: true, ..Default::default() }), vec!["clip.mp4"]);

        // Cameras, including "unknown".
        let f = Filter { cameras: vec!["X-T10".into(), "".into()], ..Default::default() };
        assert_eq!(names(&cat, &f), vec!["IMG_0003.JPG", "DSCF0001.RAF", "clip.mp4"]);

        // Keywords: all must match, any source, case-insensitive.
        let c = cat.conn();
        keywords::add(c, &[ids["DSCF0001.RAF"], ids["DSC00002.ARW"]], &["travel".into()], keywords::USER).unwrap();
        keywords::add(c, &[ids["DSCF0001.RAF"]], &["sea".into()], keywords::USER).unwrap();
        keywords::apply_labels(c, &[keywords::AssetLabels { id: ids["IMG_0003.JPG"], keywords: vec!["Sea".into()], confidences: vec![] }], "ai:claude", false).unwrap();
        assert_eq!(names(&cat, &Filter { keywords: vec!["TRAVEL".into()], ..Default::default() }), vec!["DSC00002.ARW", "DSCF0001.RAF"]);
        assert_eq!(names(&cat, &Filter { keywords: vec!["travel".into(), "sea".into()], ..Default::default() }), vec!["DSCF0001.RAF"]);
        assert_eq!(names(&cat, &Filter { ai_labelled: Some(true), ..Default::default() }), vec!["IMG_0003.JPG"]);

        // Text: file path, keyword, camera; every word must match.
        assert_eq!(names(&cat, &Filter { text: Some("beach".into()), ..Default::default() }), vec!["DSCF0001.RAF"]);
        assert_eq!(names(&cat, &Filter { text: Some("sea".into()), ..Default::default() }), vec!["IMG_0003.JPG", "DSCF0001.RAF"]);
        assert_eq!(names(&cat, &Filter { text: Some("ilce 2024".into()), ..Default::default() }), vec!["DSC00002.ARW"]);
        assert!(names(&cat, &Filter { text: Some("100%".into()), ..Default::default() }).is_empty());

        // Renders and kinds.
        let root: i64 = c.query_row("SELECT id FROM roots", [], |r| r.get(0)).unwrap();
        c.execute(
            "INSERT INTO renders (asset_id, kind, root_id, rel, preset_hash, created_at) VALUES (?1, 'jpeg', ?2, 'x.jpg', 'h', 'now')",
            rusqlite::params![ids["DSC00002.ARW"], root],
        )
        .unwrap();
        assert_eq!(names(&cat, &Filter { has_render: Some(true), ..Default::default() }), vec!["DSC00002.ARW"]);
        assert_eq!(names(&cat, &Filter { has_render: Some(false), kinds: vec!["raw".into()], ..Default::default() }), vec!["DSCF0001.RAF"]);
        assert_eq!(names(&cat, &Filter { roots: vec![root + 99], ..Default::default() }), Vec::<String>::new());
        assert_eq!(cat.ids(&Filter { kinds: vec!["video".into()], ..Default::default() }, Sort::CapturedDesc).unwrap(), vec![ids["clip.mp4"]]);

        // Facets.
        let facets = cat.facets().unwrap();
        assert_eq!(facets.total, 4);
        assert_eq!(facets.undated, 1);
        assert_eq!(facets.rendered, 1);
        assert_eq!(facets.months.first().unwrap().key, "2025-01");
        let sea = facets.keywords.iter().find(|k| k.name == "sea").unwrap();
        assert_eq!((sea.count, sea.ai), (2, 1));
        assert!(facets.cameras.iter().any(|c| c.key.is_empty() && c.count == 2));

        // Detail.
        let d = cat.detail(ids["DSCF0001.RAF"]).unwrap();
        assert_eq!(d.files.len(), 2);
        assert_eq!(d.files[0].role, "raw", "primary first");
        assert_eq!(d.files[1].role, "jpeg");
        assert!(d.files[1].path.ends_with("2024/beach/DSCF0001.JPG"));
        assert_eq!(d.keywords.len(), 2);
    }
}

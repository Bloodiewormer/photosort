use app::views::duplicates::{
    DuplicatesSummary, explorer_select_args, format_group_heading, format_summary_line,
    group_wasted_bytes, sort_groups, summarize,
};
use catalog::{AssetId, DuplicateGroup, Entry};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn make_entry(asset: i64, path: &str, sha: &str, size: u64) -> Entry {
    Entry {
        asset: AssetId(asset),
        path: PathBuf::from(path),
        sha256: sha.into(),
        size,
        modified_ns: Some(1_700_000_000_000_000_000),
    }
}

fn make_group(asset: i64, sha: &str, size: u64, paths: &[&str]) -> DuplicateGroup {
    DuplicateGroup {
        sha256: sha.into(),
        size,
        entries: paths
            .iter()
            .map(|p| make_entry(asset, p, sha, size))
            .collect(),
    }
}

#[test]
fn summarize_totals_wasted_bytes_and_redundant_copies() {
    assert_eq!(
        summarize(&[]),
        DuplicatesSummary {
            group_count: 0,
            total_copies: 0,
            redundant_copies: 0,
            wasted_bytes: 0,
        }
    );

    let g1 = make_group(1, "aaa", 1024, &["a/1.jpg", "a/2.jpg", "a/3.jpg"]); // wasted 2048
    let g2 = make_group(2, "bbb", 500, &["b/1.jpg", "b/2.jpg"]); // wasted 500
    assert_eq!(group_wasted_bytes(&g1), 2048);
    assert_eq!(group_wasted_bytes(&g2), 500);

    let summary = summarize(&[g1, g2]);
    assert_eq!(
        summary,
        DuplicatesSummary {
            group_count: 2,
            total_copies: 5,
            redundant_copies: 3,
            wasted_bytes: 2548,
        }
    );
}

#[test]
fn sort_groups_orders_by_wasted_bytes_desc_then_sha_and_entries_by_path() {
    let mut groups = vec![
        make_group(1, "zzz", 10, &["z/2.jpg", "z/1.jpg"]), // wasted 10
        make_group(2, "bbb", 20, &["b/2.jpg", "b/1.jpg"]), // wasted 20
        make_group(3, "aaa", 10, &["a/3.jpg", "a/1.jpg", "a/2.jpg"]), // wasted 20 (ties bbb)
        make_group(4, "ccc", 50, &["c/2.jpg", "c/1.jpg"]), // wasted 50
    ];

    sort_groups(&mut groups);

    let shas: Vec<&str> = groups.iter().map(|g| g.sha256.as_str()).collect();
    assert_eq!(shas, vec!["ccc", "aaa", "bbb", "zzz"]);
    let a_paths: Vec<&str> = groups[1]
        .entries
        .iter()
        .map(|e| e.path.to_str().unwrap())
        .collect();
    assert_eq!(a_paths, vec!["a/1.jpg", "a/2.jpg", "a/3.jpg"]);
}

#[test]
fn formatting_and_explorer_select_args_produce_expected_spanish_text() {
    let single = DuplicatesSummary {
        group_count: 1,
        total_copies: 2,
        redundant_copies: 1,
        wasted_bytes: 2048,
    };
    assert_eq!(
        format_summary_line(&single),
        "1 grupo · 1 copia redundante · 2.0 KB de espacio duplicado"
    );

    let multi = DuplicatesSummary {
        group_count: 3,
        total_copies: 8,
        redundant_copies: 5,
        wasted_bytes: 2 * 1024 * 1024,
    };
    assert_eq!(
        format_summary_line(&multi),
        "3 grupos · 5 copias redundantes · 2.00 MB de espacio duplicado"
    );

    let g = make_group(1, "aaa", 1024, &["x/1.jpg", "x/2.jpg", "x/3.jpg"]);
    assert_eq!(
        format_group_heading(&g),
        "3 copias · 1.0 KB c/u · 2.0 KB duplicados"
    );

    let p = Path::new(r"C:\fotos de prueba\imagen 01.jpg");
    assert_eq!(
        explorer_select_args(p),
        [
            OsString::from("/select,"),
            OsString::from(r"C:\fotos de prueba\imagen 01.jpg"),
        ]
    );
}

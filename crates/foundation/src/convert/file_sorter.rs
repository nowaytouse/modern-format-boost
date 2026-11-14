//! File Sorting Module
//!
//! 🎯 Small-file-first strategy:
//! - Quick progress feedback
//! - Small files process quickly, allowing for earlier issue detection
//! - Large files are left for later to avoid long freezes
//!
//! Modular design for easy maintenance and testing

use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size: Option<u64>,
}

impl FileInfo {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        let size = match fs::metadata(&path) {
            Ok(meta) => Some(meta.len()),
            Err(e) => {
                crate::media_conversion_gate::delivery_runtime_path_audit(
                    "delivery_runtime",
                    &path,
                    format!(
                        "SORT AUDIT: Failed to read metadata for '{}' | Forensic: Error '{}'; \
                         preserving file in list with missing size to prevent data loss during \
                         processing sweep",
                        path.display(),
                        e
                    ),
                );
                None
            }
        };
        Self { path, size }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortStrategy {
    SizeAscending,
    SizeDescending,
    NameAscending,
    None,
}

pub struct FileSorter {
    strategy: SortStrategy,
}

impl FileSorter {
    #[must_use]
    pub const fn new(strategy: SortStrategy) -> Self {
        Self { strategy }
    }

    #[must_use]
    pub fn sort(&self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        match self.strategy {
            SortStrategy::None => files,
            SortStrategy::SizeAscending => Self::sort_by_size_ascending(files),
            SortStrategy::SizeDescending => Self::sort_by_size_descending(files),
            SortStrategy::NameAscending => Self::sort_by_name(files),
        }
    }

    fn sort_by_size_ascending(files: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut file_infos: Vec<FileInfo> = files.into_iter().map(FileInfo::new).collect();

        file_infos.sort_by(|left, right| match (left.size, right.size) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.path.cmp(&right.path),
        });
        file_infos.into_iter().map(|f| f.path).collect()
    }

    fn sort_by_size_descending(files: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut file_infos: Vec<FileInfo> = files.into_iter().map(FileInfo::new).collect();

        file_infos.sort_by(|left, right| match (left.size, right.size) {
            (Some(left), Some(right)) => right.cmp(&left),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.path.cmp(&right.path),
        });
        file_infos.into_iter().map(|f| f.path).collect()
    }

    fn sort_by_name(mut files: Vec<PathBuf>) -> Vec<PathBuf> {
        files.sort();
        files
    }
}

#[must_use]
pub fn sort_by_size_ascending(files: Vec<PathBuf>) -> Vec<PathBuf> {
    FileSorter::new(SortStrategy::SizeAscending).sort(files)
}

#[must_use]
pub fn sort_by_size_descending(files: Vec<PathBuf>) -> Vec<PathBuf> {
    FileSorter::new(SortStrategy::SizeDescending).sort(files)
}

#[must_use]
pub fn sort_by_name(files: Vec<PathBuf>) -> Vec<PathBuf> {
    FileSorter::new(SortStrategy::NameAscending).sort(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;
    use tempfile::TempDir;

    fn create_test_file(dir: &Path, name: &str, size: usize) -> PathBuf {
        let path = dir.join(name);
        let mut file = fs::File::create(&path).unwrap_or_else(|e| panic!("create failed: {e:?}"));
        let data = vec![0u8; size];
        file.write_all(&data)
            .unwrap_or_else(|e| panic!("write failed: {e:?}"));
        path
    }

    #[test]
    fn test_file_info_creation() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e:?}"));
        let file_path = create_test_file(temp_dir.path(), "test.txt", 100);

        let info = FileInfo::new(file_path.clone());
        assert_eq!(info.path, file_path);
        assert_eq!(info.size, Some(100));
    }

    #[test]
    fn test_file_info_nonexistent() {
        let path = PathBuf::from("/nonexistent/file.txt");
        let info = FileInfo::new(path);
        assert_eq!(info.size, None);
    }

    #[test]
    fn test_sort_by_size_ascending() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e:?}"));

        let large = create_test_file(temp_dir.path(), "large.txt", 1000);
        let small = create_test_file(temp_dir.path(), "small.txt", 100);
        let medium = create_test_file(temp_dir.path(), "medium.txt", 500);

        let files = vec![large.clone(), small.clone(), medium.clone()];
        let sorted = sort_by_size_ascending(files);

        assert_eq!(sorted.len(), 3);
        assert_eq!(
            *sorted.first().unwrap_or_else(|| panic!("missing 0")),
            small
        );
        assert_eq!(
            *sorted.get(1).unwrap_or_else(|| panic!("missing 1")),
            medium
        );
        assert_eq!(*sorted.get(2).unwrap_or_else(|| panic!("missing 2")), large);
    }

    #[test]
    fn test_sort_by_size_descending() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e:?}"));

        let large = create_test_file(temp_dir.path(), "large.txt", 1000);
        let small = create_test_file(temp_dir.path(), "small.txt", 100);
        let medium = create_test_file(temp_dir.path(), "medium.txt", 500);

        let files = vec![small.clone(), medium.clone(), large.clone()];
        let sorted = sort_by_size_descending(files);

        assert_eq!(sorted.len(), 3);
        assert_eq!(
            *sorted.first().unwrap_or_else(|| panic!("missing 0")),
            large
        );
        assert_eq!(
            *sorted.get(1).unwrap_or_else(|| panic!("missing 1")),
            medium
        );
        assert_eq!(*sorted.get(2).unwrap_or_else(|| panic!("missing 2")), small);
    }

    #[test]
    fn test_sort_by_name() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir failed: {e:?}"));

        let a = create_test_file(temp_dir.path(), "a.txt", 100);
        let b = create_test_file(temp_dir.path(), "b.txt", 100);
        let c = create_test_file(temp_dir.path(), "c.txt", 100);

        let files = vec![c.clone(), a.clone(), b.clone()];
        let sorted = sort_by_name(files);

        assert_eq!(sorted.len(), 3);
        assert_eq!(*sorted.first().unwrap_or_else(|| panic!("missing 0")), a);
        assert_eq!(*sorted.get(1).unwrap_or_else(|| panic!("missing 1")), b);
        assert_eq!(*sorted.get(2).unwrap_or_else(|| panic!("missing 2")), c);
    }

    #[test]
    fn test_sort_strategy_none() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));

        let f1 = create_test_file(temp_dir.path(), "z.txt", 1000);
        let f2 = create_test_file(temp_dir.path(), "a.txt", 100);
        let f3 = create_test_file(temp_dir.path(), "m.txt", 500);

        let files = vec![f1, f2, f3];
        let file_sorter = FileSorter::new(SortStrategy::None);
        let sorted_results = file_sorter.sort(files.clone());

        assert_eq!(sorted_results, files);
    }

    #[test]
    fn test_empty_list() {
        let files: Vec<PathBuf> = vec![];
        let sorted = sort_by_size_ascending(files);
        assert_eq!(sorted.len(), 0);
    }

    #[test]
    fn test_single_file() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let file = create_test_file(temp_dir.path(), "single.txt", 100);

        let files = vec![file.clone()];
        let sorted = sort_by_size_ascending(files);

        assert_eq!(sorted.len(), 1);
        assert_eq!(*sorted.first().unwrap_or(&PathBuf::new()), file);
    }

    #[test]
    fn test_same_size_files() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));

        let f1 = create_test_file(temp_dir.path(), "file1.txt", 100);
        let f2 = create_test_file(temp_dir.path(), "file2.txt", 100);
        let f3 = create_test_file(temp_dir.path(), "file3.txt", 100);

        let files = vec![f1, f2, f3];
        let sorted = sort_by_size_ascending(files);

        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn test_strict_sorting_correctness() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));

        let size_samples = [5000, 100, 3000, 200, 4000, 50, 1000];
        let mut files = Vec::new();

        for (i, size) in size_samples.iter().enumerate() {
            let file = create_test_file(temp_dir.path(), &format!("file{i}.txt"), *size);
            files.push(file);
        }

        let sorted_paths = sort_by_size_ascending(files);

        for i in 0..sorted_paths.len() - 1 {
            let first_size = fs::metadata(sorted_paths.get(i).unwrap_or(&PathBuf::new()))
                .unwrap_or_else(|e| panic!("error: {e:?}"))
                .len();
            let second_size = fs::metadata(sorted_paths.get(i + 1).unwrap_or(&PathBuf::new()))
                .unwrap_or_else(|e| panic!("error: {e:?}"))
                .len();
            assert!(
                first_size <= second_size,
                "STRICT: File {} ({}B) should be <= file {} ({}B)",
                sorted_paths.get(i).unwrap_or(&PathBuf::new()).display(),
                first_size,
                sorted_paths.get(i + 1).unwrap_or(&PathBuf::new()).display(),
                second_size
            );
        }
    }
}

//! File Sorting Module
//!
//! 🎯 优先处理小文件策略：
//! - 快速看到进度反馈
//! - 小文件处理快，可以更早发现问题
//! - 大文件留到后面，避免长时间卡住
//!
//! 模块化设计，便于维护和测试

use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
}

impl FileInfo {
    pub fn new(path: PathBuf) -> Option<Self> {
        fs::metadata(&path).ok().map(|meta| FileInfo {
            path,
            size: meta.len(),
        })
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
    pub fn new(strategy: SortStrategy) -> Self {
        Self { strategy }
    }

    pub fn sort(&self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        match self.strategy {
            SortStrategy::None => files,
            SortStrategy::SizeAscending => self.sort_by_size_ascending(files),
            SortStrategy::SizeDescending => self.sort_by_size_descending(files),
            SortStrategy::NameAscending => self.sort_by_name(files),
        }
    }

    fn sort_by_size_ascending(&self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut file_infos: Vec<FileInfo> = files.into_iter().filter_map(FileInfo::new).collect();

        file_infos.sort_by_key(|f| f.size);
        file_infos.into_iter().map(|f| f.path).collect()
    }

    fn sort_by_size_descending(&self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut file_infos: Vec<FileInfo> = files.into_iter().filter_map(FileInfo::new).collect();

        file_infos.sort_by(|a, b| b.size.cmp(&a.size));
        file_infos.into_iter().map(|f| f.path).collect()
    }

    fn sort_by_name(&self, mut files: Vec<PathBuf>) -> Vec<PathBuf> {
        files.sort();
        files
    }
}

pub fn sort_by_size_ascending(files: Vec<PathBuf>) -> Vec<PathBuf> {
    FileSorter::new(SortStrategy::SizeAscending).sort(files)
}

pub fn sort_by_size_descending(files: Vec<PathBuf>) -> Vec<PathBuf> {
    FileSorter::new(SortStrategy::SizeDescending).sort(files)
}

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
        let mut file = fs::File::create(&path).unwrap();
        let data = vec![0u8; size];
        file.write_all(&data).unwrap();
        path
    }

    #[test]
    fn test_file_info_creation() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = create_test_file(temp_dir.path(), "test.txt", 100);

        let info = FileInfo::new(file_path.clone()).unwrap();
        assert_eq!(info.path, file_path);
        assert_eq!(info.size, 100);
    }

    #[test]
    fn test_file_info_nonexistent() {
        let path = PathBuf::from("/nonexistent/file.txt");
        let info = FileInfo::new(path);
        assert!(info.is_none());
    }

    #[test]
    fn test_sort_by_size_ascending() {
        let temp_dir = TempDir::new().unwrap();

        let large = create_test_file(temp_dir.path(), "large.txt", 1000);
        let small = create_test_file(temp_dir.path(), "small.txt", 100);
        let medium = create_test_file(temp_dir.path(), "medium.txt", 500);

        let files = vec![large.clone(), small.clone(), medium.clone()];
        let sorted = sort_by_size_ascending(files);

        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0], small);
        assert_eq!(sorted[1], medium);
        assert_eq!(sorted[2], large);
    }

    #[test]
    fn test_sort_by_size_descending() {
        let temp_dir = TempDir::new().unwrap();

        let large = create_test_file(temp_dir.path(), "large.txt", 1000);
        let small = create_test_file(temp_dir.path(), "small.txt", 100);
        let medium = create_test_file(temp_dir.path(), "medium.txt", 500);

        let files = vec![small.clone(), medium.clone(), large.clone()];
        let sorted = sort_by_size_descending(files);

        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0], large);
        assert_eq!(sorted[1], medium);
        assert_eq!(sorted[2], small);
    }

    #[test]
    fn test_sort_by_name() {
        let temp_dir = TempDir::new().unwrap();

        let c = create_test_file(temp_dir.path(), "c.txt", 100);
        let a = create_test_file(temp_dir.path(), "a.txt", 100);
        let b = create_test_file(temp_dir.path(), "b.txt", 100);

        let files = vec![c.clone(), a.clone(), b.clone()];
        let sorted = sort_by_name(files);

        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0], a);
        assert_eq!(sorted[1], b);
        assert_eq!(sorted[2], c);
    }

    #[test]
    fn test_sort_strategy_none() {
        let temp_dir = TempDir::new().unwrap();

        let f1 = create_test_file(temp_dir.path(), "z.txt", 1000);
        let f2 = create_test_file(temp_dir.path(), "a.txt", 100);
        let f3 = create_test_file(temp_dir.path(), "m.txt", 500);

        let files = vec![f1.clone(), f2.clone(), f3.clone()];
        let sorter = FileSorter::new(SortStrategy::None);
        let sorted = sorter.sort(files.clone());

        assert_eq!(sorted, files);
    }

    #[test]
    fn test_empty_list() {
        let files: Vec<PathBuf> = vec![];
        let sorted = sort_by_size_ascending(files);
        assert!(sorted.is_empty());
    }

    #[test]
    fn test_single_file() {
        let temp_dir = TempDir::new().unwrap();
        let file = create_test_file(temp_dir.path(), "single.txt", 100);

        let files = vec![file.clone()];
        let sorted = sort_by_size_ascending(files);

        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0], file);
    }

    #[test]
    fn test_same_size_files() {
        let temp_dir = TempDir::new().unwrap();

        let f1 = create_test_file(temp_dir.path(), "file1.txt", 100);
        let f2 = create_test_file(temp_dir.path(), "file2.txt", 100);
        let f3 = create_test_file(temp_dir.path(), "file3.txt", 100);

        let files = vec![f1.clone(), f2.clone(), f3.clone()];
        let sorted = sort_by_size_ascending(files);

        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn test_strict_sorting_correctness() {
        let temp_dir = TempDir::new().unwrap();

        let sizes = [5000, 100, 3000, 200, 4000, 50, 1000];
        let mut files = Vec::new();

        for (i, size) in sizes.iter().enumerate() {
            let file = create_test_file(temp_dir.path(), &format!("file{}.txt", i), *size);
            files.push(file);
        }

        let sorted = sort_by_size_ascending(files);

        for i in 0..sorted.len() - 1 {
            let size1 = fs::metadata(&sorted[i]).unwrap().len();
            let size2 = fs::metadata(&sorted[i + 1]).unwrap().len();
            assert!(
                size1 <= size2,
                "STRICT: File {} ({}B) should be <= file {} ({}B)",
                sorted[i].display(),
                size1,
                sorted[i + 1].display(),
                size2
            );
        }
    }
}

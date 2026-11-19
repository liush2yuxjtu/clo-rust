use serde::Serialize;
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path},
};

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    MatchesBundle,
    Different,
    BundleOnly,
    LibraryOnly,
    Missing,
}

#[derive(Serialize)]
pub struct AssetCheck<'a> {
    path: &'a str,
    bundle_bytes: Option<u64>,
    library_bytes: Option<u64>,
    comparison: Comparison,
}

fn open(path: &Path) -> Result<Option<(File, u64)>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}（{error}）", path.display())),
    };
    if !metadata.is_file() {
        return Err(format!("需要普通资源文件：{}", path.display()));
    }
    let file = File::open(path).map_err(|error| format!("{}（{error}）", path.display()))?;
    Ok(Some((file, metadata.len())))
}

fn equal_bytes(left: &mut File, right: &mut File, length: u64) -> io::Result<bool> {
    let mut a = [0; 65536];
    let mut b = [0; 65536];
    let mut remaining = length;
    while remaining > 0 {
        let chunk = remaining.min(a.len() as u64) as usize;
        left.read_exact(&mut a[..chunk])?;
        right.read_exact(&mut b[..chunk])?;
        if a[..chunk] != b[..chunk] {
            return Ok(false);
        }
        remaining -= chunk as u64;
    }
    Ok(left.read(&mut a[..1])? == 0 && right.read(&mut b[..1])? == 0)
}

pub fn compare<'a>(
    bundle: &str,
    library: &str,
    assets: &[&'a str],
) -> Result<Vec<AssetCheck<'a>>, String> {
    let (bundle, library) = (Path::new(bundle), Path::new(library));
    for root in [bundle, library] {
        if !root.is_dir() {
            return Err(format!("资产目录不可读：{}", root.display()));
        }
    }
    assets
        .iter()
        .map(|&path| {
            let item = Path::new(path);
            if item.as_os_str().is_empty()
                || item
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(format!("需要资产目录中的相对文件路径：{path}"));
            }
            let bundled = open(&bundle.join(item))?;
            let installed = open(&library.join(item))?;
            let bundle_bytes = bundled.as_ref().map(|entry| entry.1);
            let library_bytes = installed.as_ref().map(|entry| entry.1);
            let comparison = match (bundled, installed) {
                (Some((mut original, size)), Some((mut working, working_size))) => {
                    if size == working_size
                        && equal_bytes(&mut original, &mut working, size)
                            .map_err(|error| format!("资源比较失败：{path}（{error}）"))?
                    {
                        Comparison::MatchesBundle
                    } else {
                        Comparison::Different
                    }
                }
                (Some(_), None) => Comparison::BundleOnly,
                (None, Some(_)) => Comparison::LibraryOnly,
                (None, None) => Comparison::Missing,
            };
            Ok(AssetCheck {
                path,
                bundle_bytes,
                library_bytes,
                comparison,
            })
        })
        .collect()
}

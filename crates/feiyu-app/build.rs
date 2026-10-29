//! 把 `resources/icon.ico` 编进 exe 的资源段（Explorer 里看到的那个图标）。
//!
//! 不用 winres / embed-resource：一个多依赖、一个要求 rc.exe 在 PATH 里。
//! 这里自己找 Windows SDK 的 rc.exe，生成 .res，再作为链接参数交给 link.exe。
//! 找不到只打警告——没有图标不影响运行，不能因此让整个构建失败。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=resources/icon.ico");

    #[cfg(windows)]
    if let Err(e) = embed_icon() {
        println!("cargo:warning=嵌入 exe 图标失败（不影响运行）：{e}");
    }
}

#[cfg(windows)]
fn embed_icon() -> Result<(), String> {
    let rc = find_rc().ok_or_else(|| "找不到 rc.exe（需要 Windows SDK）".to_owned())?;
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let res = out_dir.join("app.res");
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg("resources/app.rc")
        .status()
        .map_err(|e| format!("无法启动 {}：{e}", rc.display()))?;
    if !status.success() {
        return Err(format!("rc.exe 退出码 {:?}", status.code()));
    }
    // 交给 link.exe：.res 是链接器的合法输入。
    println!("cargo:rustc-link-arg-bins={}", res.display());
    Ok(())
}

/// 先找 PATH，再找 Windows Kits\10\bin\<version>\x64\rc.exe（取版本号最大的）。
#[cfg(windows)]
fn find_rc() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let exe = dir.join("rc.exe");
            if exe.is_file() {
                return Some(exe);
            }
        }
    }

    let mut roots = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(base) = std::env::var_os(var) {
            roots.push(Path::new(&base).join("Windows Kits").join("10").join("bin"));
        }
    }

    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let key: Vec<u32> = name.split('.').filter_map(|p| p.parse().ok()).collect();
            if key.is_empty() {
                continue;
            }
            let exe = entry.path().join("x64").join("rc.exe");
            if !exe.is_file() {
                continue;
            }
            let better = match &best {
                Some((k, _)) => key > *k,
                None => true,
            };
            if better {
                best = Some((key, exe));
            }
        }
    }
    best.map(|(_, p)| p)
}

//! Embeds the app icon as resource 1, which gpui loads for every window (taskbar, Alt-Tab,
//! title bar) and Explorer shows for the exe. It uses the Windows SDK's resource compiler,
//! found next to the fxc.exe the gpui build already needs, and hands the .res to the linker,
//! so no extra crate is involved.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/shoter.ico");
    println!("cargo:rerun-if-changed=assets/shoter.rc");
    println!("cargo:rerun-if-env-changed=GPUI_FXC_PATH");
    if env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    let rc = env::var_os("GPUI_FXC_PATH")
        .map(|p| PathBuf::from(p).with_file_name("rc.exe"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("rc.exe"));
    let res = PathBuf::from(env::var("OUT_DIR").unwrap()).join("shoter.res");
    let built = Command::new(&rc)
        .current_dir("assets")
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg("shoter.rc")
        .status()
        .is_ok_and(|s| s.success());
    if built {
        println!("cargo:rustc-link-arg-bins={}", res.display());
    } else {
        println!("cargo:warning=rc.exe not found or failed: the exe is built without its icon");
    }
}

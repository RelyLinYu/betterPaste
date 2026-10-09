// Windows: hide the console window for release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::Command;

fn main() {
    tauri_build::build();

    // `tauri-build` only links its resource (which carries the Common-Controls v6
    // manifest) into the *bin* target. Test executables therefore start without
    // that manifest, so `TaskDialogIndirect` — pulled in from comctl32 by Tauri —
    // cannot be resolved and the process dies with STATUS_ENTRYPOINT_NOT_FOUND.
    //
    // Cargo does not expose "is this a test build?" to build scripts (PROFILE is
    // `debug` for both `cargo build` and `cargo test`, and CARGO_CFG_TEST is unset),
    // so the tiny extra resource is added unconditionally. For the bin target the
    // linker merges it with tauri-build's resource; the worst case is a
    // "multiple non-default manifests" warning, which GNU ld already emits anyway.
    if let Err(e) = embed_manifest_resource() {
        println!("cargo:warning=could not embed the Common-Controls manifest: {e}");
    }
}

fn embed_manifest_resource() -> Result<(), String> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let windres = find_tool("windres")?;
    let ar = find_tool("ar")?;

    let rc = out_dir.join("clipstack-test.rc");
    std::fs::write(
        &rc,
        r#"1 24
{
  " <assembly xmlns=""urn:schemas-microsoft-com:asm.v1"" manifestVersion=""1.0""> "
  "  <dependency> "
  "    <dependentAssembly> "
  "      <assemblyIdentity type=""win32"" name=""Microsoft.Windows.Common-Controls"" "
  "        version=""6.0.0.0"" processorArchitecture=""*"" "
  "        publicKeyToken=""6595b64144ccf1df"" language=""*"" /> "
  "    </dependentAssembly> "
  "  </dependency> "
  " </assembly> "
}
"#,
    )
    .map_err(|e| e.to_string())?;

    let obj = out_dir.join("clipstack-test-manifest.o");
    let status = Command::new(&windres)
        .arg("-I")
        .arg(&out_dir)
        .arg(&rc)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&obj)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("windres failed".into());
    }

    // Pass the object straight to the linker. Putting it in a static archive does
    // NOT work: the object defines no symbols, so GNU ld never pulls it in and the
    // manifest silently disappears.
    println!("cargo:rustc-link-arg={}", obj.display());
    Ok(())
}

/// MinGW tools live next to the linker rustc is configured to use; the build
/// environment does not necessarily have them on PATH.
fn find_tool(name: &str) -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER") {
        if let Some(dir) = PathBuf::from(p).parent() {
            for candidate in [
                format!("{name}.exe"),
                format!("x86_64-w64-mingw32-{name}.exe"),
            ] {
                let path = dir.join(candidate);
                if path.exists() {
                    return Ok(path);
                }
            }
        }
    }

    let which = if cfg!(windows) { "where" } else { "which" };
    let output = Command::new(which).arg(name).output().map_err(|e| e.to_string())?;
    if output.status.success() {
        let first = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if !first.is_empty() {
            return Ok(PathBuf::from(first));
        }
    }
    Err(format!("{name} not found"))
}

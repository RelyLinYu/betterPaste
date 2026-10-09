// Windows: hide the console window for release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // 诊断入口：clipstack --probe [搜索词] [--bench N]
        Some("--probe") => {
            let rest = &args[1..];
            let bench = rest
                .iter()
                .position(|a| a == "--bench")
                .and_then(|i| rest.get(i + 1))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let query = rest
                .iter()
                .find(|a| !a.starts_with("--") && a.parse::<usize>().is_err())
                .cloned();
            clipstack_lib::config::probe(query, bench);
        }
        // 运维入口：clipstack --ctl pin <1-9> <搜索词> | --ctl unpin <1-9>
        Some("--ctl") => match args.get(1).map(String::as_str) {
            Some("pin") => {
                let slot = args.get(2).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let needle = args.get(3).cloned().unwrap_or_default();
                let ok = clipstack_lib::config::ctl_pin(slot, &needle);
                std::process::exit(if ok { 0 } else { 1 });
            }
            Some("unpin") => {
                let slot = args.get(2).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let ok = clipstack_lib::config::ctl_unpin(slot);
                std::process::exit(if ok { 0 } else { 1 });
            }
            _ => {
                println!("用法: clipstack --ctl pin <1-9> <搜索词> | clipstack --ctl unpin <1-9>");
                std::process::exit(2);
            }
        },
        // 启动后直接打开设置面板
        Some("--settings") => clipstack_lib::run_with(clipstack_lib::StartWindow::Settings),
        // 启动后直接呼出浮层
        Some("--overlay") => clipstack_lib::run_with(clipstack_lib::StartWindow::Overlay),
        _ => clipstack_lib::run(),
    }
}

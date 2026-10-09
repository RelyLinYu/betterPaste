// Windows: hide the console window for release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // 诊断入口：clipstack --probe [搜索词] [--bench N]
        Some("--probe") => {
            let rest = &args[1..];
            // `--bench <N>` 先按位置摘掉；其余非 `--` 开头的参数就是搜索词。
            // 旧写法把「能 parse 成 usize」当参数特征，于是 `--probe 123`（工单号/错误码）
            // 会被静默丢掉，改成打印最近 20 条并显示"匹配 N 条"。
            let mut bench = 0usize;
            let mut query: Option<String> = None;
            let mut i = 0;
            while i < rest.len() {
                let a = &rest[i];
                if a == "--bench" {
                    bench = rest.get(i + 1).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                    i += 2;
                    continue;
                }
                if !a.starts_with("--") && query.is_none() {
                    query = Some(a.clone());
                }
                i += 1;
            }
            clipstack_lib::config::probe(query, bench);
        }
        // 运维入口：clipstack --ctl pin <1-9> <搜索词> | --ctl unpin <1-9>
        Some("--ctl") => match args.get(1).map(String::as_str) {
            Some("pin") => {
                let slot = args.get(2).and_then(|s| s.parse::<u8>().ok());
                let needle = args.get(3).cloned().unwrap_or_default();
                let Some(slot) = slot.filter(|s| (1..=9).contains(s)) else {
                    println!("用法: clipstack --ctl pin <1-9> <搜索词>");
                    std::process::exit(2);
                };
                let ok = clipstack_lib::config::ctl_pin(slot, &needle);
                std::process::exit(if ok { 0 } else { 1 });
            }
            Some("unpin") => {
                // 之前槽位解析失败会静默变成槽 0，然后打印"槽 0 本来就是空的"并 exit 0
                let slot = args.get(2).and_then(|s| s.parse::<u8>().ok());
                let Some(slot) = slot.filter(|s| (1..=9).contains(s)) else {
                    println!("用法: clipstack --ctl unpin <1-9>");
                    std::process::exit(2);
                };
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

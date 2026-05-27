use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// ── 数据结构 ──────────────────────────────────────────────────────────────────

struct FileIndex {
    /// rel_path → file_size
    by_path: HashMap<String, u64>,
    /// "basename:size" → Vec<rel_path>（支持多匹配）
    by_name_size: HashMap<String, Vec<String>>,
}

impl FileIndex {
    fn new() -> Self {
        FileIndex {
            by_path: HashMap::new(),
            by_name_size: HashMap::new(),
        }
    }

    fn insert(&mut self, rel_path: String, size: u64) {
        let bn = Path::new(&rel_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let key = format!("{}:{}", bn, size);
        self.by_name_size.entry(key).or_default().push(rel_path.clone());
        self.by_path.insert(rel_path, size);
    }
}

struct SyncPlan {
    /// (b_old_rel, b_new_rel)：B 内部移动
    moves: Vec<(String, String)>,
    /// 从 A 复制到 B 的相对路径
    copies: Vec<String>,
    /// 移到 trash 的 B 相对路径
    trashes: Vec<String>,
    skip_count: usize,
    total_copy_bytes: u64,
}

// ── 扫描 ──────────────────────────────────────────────────────────────────────

fn scan_dir(root: &Path) -> io::Result<FileIndex> {
    let mut index = FileIndex::new();
    let root_str = root.to_string_lossy().to_string();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let abs = entry.path().to_string_lossy().to_string();
        if abs.contains("/_trash_") {
            continue;
        }
        let rel = abs
            .strip_prefix(&format!("{}/", root_str))
            .unwrap_or(&abs)
            .to_string();
        let size = entry.metadata()?.len();
        index.insert(rel, size);
    }

    Ok(index)
}

// ── 比对 + 移动识别 ───────────────────────────────────────────────────────────

fn build_plan(a_idx: &FileIndex, b_idx: &FileIndex) -> SyncPlan {
    let mut candidate_copies: Vec<String> = Vec::new();
    let mut candidate_trashes: Vec<String> = Vec::new();
    let mut trash_set: HashSet<String> = HashSet::new();
    let mut skip_count = 0usize;

    // 遍历 A：分类每个文件
    for (rel, &a_size) in &a_idx.by_path {
        match b_idx.by_path.get(rel) {
            Some(&b_size) if b_size == a_size => {
                skip_count += 1;
            }
            Some(_) => {
                // 路径相同但大小不同：需要更新
                candidate_copies.push(rel.clone());
                candidate_trashes.push(rel.clone());
                trash_set.insert(rel.clone());
            }
            None => {
                candidate_copies.push(rel.clone());
            }
        }
    }

    // 遍历 B：找 A 中不存在的文件
    for rel in b_idx.by_path.keys() {
        if !a_idx.by_path.contains_key(rel) && !trash_set.contains(rel) {
            candidate_trashes.push(rel.clone());
        }
    }

    // 移动识别：贪心匹配（排序保证确定性）
    candidate_copies.sort();
    let mut b_consumed: HashSet<String> = HashSet::new();
    let mut moves: Vec<(String, String)> = Vec::new();
    let mut new_copies: Vec<String> = Vec::new();

    for copy_path in &candidate_copies {
        let a_size = a_idx.by_path[copy_path];
        let bn = Path::new(copy_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let key = format!("{}:{}", bn, a_size);

        let matched = b_idx
            .by_name_size
            .get(&key)
            .and_then(|candidates| candidates.iter().find(|c| !b_consumed.contains(*c)))
            .cloned();

        if let Some(src_b) = matched {
            b_consumed.insert(src_b.clone());
            moves.push((src_b, copy_path.clone()));
        } else {
            new_copies.push(copy_path.clone());
        }
    }

    // 过滤 to_trash：移除已被移动识别消费的路径
    let trashes: Vec<String> = candidate_trashes
        .into_iter()
        .filter(|p| !b_consumed.contains(p))
        .collect();

    let total_copy_bytes: u64 = new_copies
        .iter()
        .map(|p| a_idx.by_path.get(p).copied().unwrap_or(0))
        .sum();

    SyncPlan { moves, copies: new_copies, trashes, skip_count, total_copy_bytes }
}

// ── 格式化工具 ────────────────────────────────────────────────────────────────

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{} B", bytes)
    }
}

// ── 展示计划 ──────────────────────────────────────────────────────────────────

fn display_plan(plan: &SyncPlan, a_idx: &FileIndex, a_root: &Path, b_root: &Path, timestamp: &str) {
    const MAX_DISPLAY: usize = 20;
    let divider = "══════════════════════════════════════════════════════";

    println!();
    println!("{}", divider.bold());
    println!(
        "{} {}  →  {}",
        " 同步计划：".bold(),
        a_root.display().to_string().cyan(),
        b_root.display().to_string().cyan()
    );
    println!("{}", divider.bold());

    // 移动
    println!();
    if plan.moves.is_empty() {
        println!("{}", "[移动] 无 B 内部移动".dimmed());
    } else {
        println!(
            "{}",
            format!("[移动] B 内部移动 {} 个文件（零跨盘消耗）", plan.moves.len())
                .green()
                .bold()
        );
        for (src, dst) in plan.moves.iter().take(MAX_DISPLAY) {
            println!("  {}  {}  →  {}", "mv".green(), src.dimmed(), dst);
        }
        if plan.moves.len() > MAX_DISPLAY {
            println!("  {}", format!("... 还有 {} 个", plan.moves.len() - MAX_DISPLAY).dimmed());
        }
    }

    // 新增/复制
    println!();
    if plan.copies.is_empty() {
        println!("{}", "[新增] 无需从 A 复制".dimmed());
    } else {
        println!(
            "{}",
            format!(
                "[新增] 从 A 复制到 B：{} 个文件，共 {}（跨盘操作）",
                plan.copies.len(),
                format_bytes(plan.total_copy_bytes)
            )
            .yellow()
            .bold()
        );
        for rel in plan.copies.iter().take(MAX_DISPLAY) {
            let size = a_idx.by_path.get(rel).copied().unwrap_or(0);
            println!(
                "  {}  {}  {}",
                "cp".yellow(),
                rel,
                format!("({})", format_bytes(size)).dimmed()
            );
        }
        if plan.copies.len() > MAX_DISPLAY {
            println!("  {}", format!("... 还有 {} 个", plan.copies.len() - MAX_DISPLAY).dimmed());
        }
    }

    // 删除/trash
    println!();
    if plan.trashes.is_empty() {
        println!("{}", "[删除] 无需移到回收站".dimmed());
    } else {
        println!(
            "{}",
            format!(
                "[删除] 移到回收站：{} 个文件 → _trash_{}/",
                plan.trashes.len(),
                timestamp
            )
            .red()
            .bold()
        );
        for rel in plan.trashes.iter().take(MAX_DISPLAY) {
            println!("  {}  {}", "trash".red(), rel);
        }
        if plan.trashes.len() > MAX_DISPLAY {
            println!("  {}", format!("... 还有 {} 个", plan.trashes.len() - MAX_DISPLAY).dimmed());
        }
    }

    // 跳过
    println!();
    println!("{}", format!("[跳过] {} 个文件已同步，无需操作", plan.skip_count).dimmed());

    // 统计
    println!();
    println!("{}", "── 统计 ────────────────────────────────────────────────".bold());
    println!("  跨盘复制：{} 个文件 / {}", plan.copies.len(), format_bytes(plan.total_copy_bytes));
    println!("  B 内部移动：{} 个文件", plan.moves.len());
    println!("  B 内部 trash：{} 个文件", plan.trashes.len());
    println!("{}", divider.bold());
}

// ── 执行 ──────────────────────────────────────────────────────────────────────

fn execute_moves(
    plan: &SyncPlan,
    b_root: &Path,
    timestamp: &str,
    inline_trashed: &mut HashSet<String>,
) -> io::Result<()> {
    if plan.moves.is_empty() {
        return Ok(());
    }
    println!("{}", format!("正在执行 B 内部移动（{} 个）...", plan.moves.len()).cyan());
    let trash_dir = b_root.join(format!("_trash_{}", timestamp));

    for (src_rel, dst_rel) in &plan.moves {
        let src = b_root.join(src_rel);
        let dst = b_root.join(dst_rel);

        // 若目标路径在 B 中已有旧文件，先 trash 它
        if dst.exists() && !inline_trashed.contains(dst_rel) {
            let trash_dst = trash_dir.join(dst_rel);
            fs::create_dir_all(trash_dst.parent().unwrap())?;
            fs::rename(&dst, &trash_dst)?;
            inline_trashed.insert(dst_rel.clone());
        }

        fs::create_dir_all(dst.parent().unwrap())?;
        fs::rename(&src, &dst)?;
        println!("  {}  {}  →  {}", "mv".green(), src_rel.dimmed(), dst_rel);
    }
    Ok(())
}

fn execute_trashes(
    plan: &SyncPlan,
    b_root: &Path,
    timestamp: &str,
    inline_trashed: &HashSet<String>,
) -> io::Result<()> {
    if plan.trashes.is_empty() {
        return Ok(());
    }
    let trash_dir = b_root.join(format!("_trash_{}", timestamp));
    println!(
        "{}",
        format!("正在移到回收站（{} 个）→ _trash_{}/", plan.trashes.len(), timestamp).cyan()
    );

    for rel in &plan.trashes {
        if inline_trashed.contains(rel) {
            continue;
        }
        let src = b_root.join(rel);
        if !src.exists() {
            continue;
        }
        let dst = trash_dir.join(rel);
        fs::create_dir_all(dst.parent().unwrap())?;
        fs::rename(&src, &dst)?;
        println!("  {}  {}", "trash".red(), rel);
    }
    Ok(())
}

fn execute_copies(plan: &SyncPlan, a_root: &Path, b_root: &Path, a_idx: &FileIndex) -> io::Result<()> {
    if plan.copies.is_empty() {
        return Ok(());
    }
    println!("{}", format!("正在从 A 复制到 B（{} 个文件）...", plan.copies.len()).cyan());

    for rel in &plan.copies {
        let src = a_root.join(rel);
        let dst = b_root.join(rel);
        fs::create_dir_all(dst.parent().unwrap())?;
        fs::copy(&src, &dst)?;
        let size = a_idx.by_path.get(rel).copied().unwrap_or(0);
        println!(
            "  {}  {}  {}",
            "cp".yellow(),
            rel,
            format!("({})", format_bytes(size)).dimmed()
        );
    }
    Ok(())
}

// ── 日期计算（不依赖外部 crate）──────────────────────────────────────────────

fn epoch_to_ymd(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

fn make_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (y, m, d) = epoch_to_ymd(secs / 86400);
    let rem = secs % 86400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", y, m, d, hh, mm, ss)
}

// ── 主流程 ────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("{}", "用法: sync-folder /path/to/A /path/to/B".red());
        std::process::exit(1);
    }

    let a_root = PathBuf::from(&args[1]);
    let b_root = PathBuf::from(&args[2]);

    if !a_root.is_dir() {
        eprintln!("{}", format!("源目录不存在：{}", a_root.display()).red());
        std::process::exit(1);
    }
    if !b_root.is_dir() {
        eprintln!("{}", format!("目标目录不存在：{}", b_root.display()).red());
        std::process::exit(1);
    }

    let real_a = fs::canonicalize(&a_root).unwrap_or_else(|_| a_root.clone());
    let real_b = fs::canonicalize(&b_root).unwrap_or_else(|_| b_root.clone());
    if real_a == real_b {
        eprintln!("{}", "源目录和目标目录不能是同一个目录".red());
        std::process::exit(1);
    }

    // 阶段一：扫描
    println!("{}", format!("正在扫描 A：{} ...", a_root.display()).cyan());
    let a_idx = scan_dir(&a_root).unwrap_or_else(|e| {
        eprintln!("{}", format!("扫描 A 失败：{}", e).red());
        std::process::exit(1);
    });

    println!("{}", format!("正在扫描 B：{} ...", b_root.display()).cyan());
    let b_idx = scan_dir(&b_root).unwrap_or_else(|e| {
        eprintln!("{}", format!("扫描 B 失败：{}", e).red());
        std::process::exit(1);
    });

    println!(
        "{}",
        format!(
            "扫描完成：A 共 {} 个文件，B 共 {} 个文件",
            a_idx.by_path.len(),
            b_idx.by_path.len()
        )
        .cyan()
    );

    // 阶段二+三：比对 + 移动识别
    let plan = build_plan(&a_idx, &b_idx);
    let timestamp = make_timestamp();

    // 展示计划
    display_plan(&plan, &a_idx, &a_root, &b_root, &timestamp);

    if plan.moves.is_empty() && plan.copies.is_empty() && plan.trashes.is_empty() {
        println!();
        println!("{}", "两个文件夹已完全同步，无需任何操作。".green());
        return;
    }

    // 等待用户确认
    println!();
    print!("{}", "输入 y 确认执行，n 取消：".bold());
    io::stdout().flush().unwrap();

    let mut input = String::new();
    loop {
        input.clear();
        io::stdin().read_line(&mut input).unwrap();
        match input.trim().to_lowercase().as_str() {
            "y" | "yes" => break,
            "n" | "no" => {
                println!();
                println!("{}", "已取消，未执行任何操作。".yellow());
                return;
            }
            _ => {
                print!("{}", "请输入 y 或 n：".yellow());
                io::stdout().flush().unwrap();
            }
        }
    }

    println!();

    // 阶段四：执行（移动 → trash → 复制）
    let mut inline_trashed: HashSet<String> = HashSet::new();

    if let Err(e) = execute_moves(&plan, &b_root, &timestamp, &mut inline_trashed) {
        eprintln!("{}", format!("移动操作失败：{}", e).red());
        std::process::exit(1);
    }
    if let Err(e) = execute_trashes(&plan, &b_root, &timestamp, &inline_trashed) {
        eprintln!("{}", format!("Trash 操作失败：{}", e).red());
        std::process::exit(1);
    }
    if let Err(e) = execute_copies(&plan, &a_root, &b_root, &a_idx) {
        eprintln!("{}", format!("复制操作失败：{}", e).red());
        std::process::exit(1);
    }

    println!();
    println!("{}", "同步完成！".green().bold());
    if !plan.trashes.is_empty() {
        println!(
            "{}",
            format!("已删除的文件移至：{}/_trash_{}/", b_root.display(), timestamp).cyan()
        );
    }
}

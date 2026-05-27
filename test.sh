#!/usr/bin/env bash
# test.sh — sync-folder 集成测试
# 用法: bash test.sh

set -euo pipefail

BINARY="./target/release/sync-folder"
WORK_DIR="$(mktemp -d)"
PASS=0
FAIL=0

# ── 工具 ──────────────────────────────────────────────────────────────────────

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BOLD='\033[1m'; NC='\033[0m'

pass() { echo -e "  ${GREEN}PASS${NC} $1"; (( PASS++ )) || true; }
fail() { echo -e "  ${RED}FAIL${NC} $1"; (( FAIL++ )) || true; }

# 断言文件存在
assert_file() {
    local path="$1" desc="$2"
    if [[ -f "$path" ]]; then pass "$desc"; else fail "$desc (缺少: $path)"; fi
}

# 断言文件不存在
assert_no_file() {
    local path="$1" desc="$2"
    if [[ ! -f "$path" ]]; then pass "$desc"; else fail "$desc (不应存在: $path)"; fi
}

# 断言文件大小匹配
assert_size() {
    local path="$1" expected="$2" desc="$3"
    local actual
    actual=$(wc -c < "$path" | tr -d ' ')
    if [[ "$actual" == "$expected" ]]; then
        pass "$desc"
    else
        fail "$desc (期望 ${expected}B，实际 ${actual}B)"
    fi
}

# 断言 trash 目录中存在某文件（路径含通配符匹配 timestamp）
assert_in_trash() {
    local b_root="$1" rel="$2" desc="$3"
    if ls "${b_root}/_trash_"*"/${rel}" 2>/dev/null | grep -q .; then
        pass "$desc"
    else
        fail "$desc (trash 中找不到: ${rel})"
    fi
}

# 运行同步（自动输入 y 确认）
run_sync() {
    local a="$1" b="$2"
    echo "y" | "$BINARY" "$a" "$b" > /dev/null 2>&1
}

# 清理并创建测试目录
setup() {
    local name="$1"
    local a="${WORK_DIR}/${name}_A"
    local b="${WORK_DIR}/${name}_B"
    rm -rf "$a" "$b"
    mkdir -p "$a" "$b"
    echo "$a $b"
}

cleanup() {
    rm -rf "$WORK_DIR"
}

trap cleanup EXIT

# ── 前置检查 ──────────────────────────────────────────────────────────────────

echo -e "${BOLD}== 前置检查 ==${NC}"
if [[ ! -f "$BINARY" ]]; then
    echo -e "${YELLOW}未找到 release 二进制，正在编译...${NC}"
    cargo build --release -q
fi
pass "二进制存在：$BINARY"

# ── 测试 1：基础新增 ──────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 1：基础新增（A 有文件，B 为空）==${NC}"
read -r A B <<< "$(setup t1)"
printf '%.0sx' {1..500} > "${A}/hello.txt"
run_sync "$A" "$B"
assert_file "${B}/hello.txt" "文件被复制到 B"
assert_size "${B}/hello.txt" 500 "文件大小正确"

# ── 测试 2：已同步跳过 ────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 2：已同步文件跳过（路径+大小相同）==${NC}"
read -r A B <<< "$(setup t2)"
printf '%.0sx' {1..300} > "${A}/same.txt"
printf '%.0sx' {1..300} > "${B}/same.txt"
# 记录 B 中文件的修改时间，确认没被重写
mtime_before=$(stat -f%m "${B}/same.txt" 2>/dev/null || stat -c%Y "${B}/same.txt")
run_sync "$A" "$B"
mtime_after=$(stat -f%m "${B}/same.txt" 2>/dev/null || stat -c%Y "${B}/same.txt")
if [[ "$mtime_before" == "$mtime_after" ]]; then
    pass "相同文件未被覆盖（mtime 未变）"
else
    fail "相同文件被重复复制（mtime 已变）"
fi

# ── 测试 3：内容更新（同路径不同大小）────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 3：文件更新（同路径，A 更新，大小不同）==${NC}"
read -r A B <<< "$(setup t3)"
mkdir -p "${A}/docs" "${B}/docs"
printf '%.0sx' {1..800} > "${A}/docs/readme.txt"  # A 新版本 800B
printf '%.0sx' {1..500} > "${B}/docs/readme.txt"  # B 旧版本 500B
run_sync "$A" "$B"
assert_size "${B}/docs/readme.txt" 800 "B 中文件已更新为新版本（800B）"
assert_in_trash "$B" "docs/readme.txt" "旧版本移入 trash"

# ── 测试 4：B 内部移动识别 ────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 4：移动识别（文件在 B 中换了目录）==${NC}"
read -r A B <<< "$(setup t4)"
mkdir -p "${A}/new_folder" "${B}/old_folder"
printf '%.0sx' {1..1000} > "${A}/new_folder/photo.jpg"
printf '%.0sx' {1..1000} > "${B}/old_folder/photo.jpg"  # 同名同大小，路径不同
run_sync "$A" "$B"
assert_file "${B}/new_folder/photo.jpg" "文件移动到新路径"
assert_no_file "${B}/old_folder/photo.jpg" "旧路径文件已消失"
# 移动不应触发 trash（旧路径是移动的源，不是删除）
if ls "${B}/_trash_"*/old_folder/photo.jpg 2>/dev/null | grep -q .; then
    fail "移动的源文件不应进入 trash"
else
    pass "移动的源文件未进入 trash（正确）"
fi

# ── 测试 5：B 中多余文件进 trash ─────────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 5：B 中多余文件移入 trash ==${NC}"
read -r A B <<< "$(setup t5)"
printf '%.0sx' {1..100} > "${A}/keep.txt"
printf '%.0sx' {1..100} > "${B}/keep.txt"
printf '%.0sx' {1..200} > "${B}/obsolete.txt"  # A 中没有
run_sync "$A" "$B"
assert_file "${B}/keep.txt" "共同文件保留"
assert_no_file "${B}/obsolete.txt" "多余文件从 B 根目录消失"
assert_in_trash "$B" "obsolete.txt" "多余文件进入 trash"

# ── 测试 6：移动歧义（多个同名同大小文件）────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 6：移动歧义（B 中两个同名同大小文件）==${NC}"
read -r A B <<< "$(setup t6)"
mkdir -p "${A}/target" "${B}/old1" "${B}/old2"
printf '%.0sx' {1..600} > "${A}/target/data.bin"
printf '%.0sx' {1..600} > "${B}/old1/data.bin"   # 同名同大小
printf '%.0sx' {1..600} > "${B}/old2/data.bin"   # 同名同大小（歧义）
run_sync "$A" "$B"
assert_file "${B}/target/data.bin" "目标路径文件存在"
# 两个旧文件：一个被移动，另一个进 trash
local_trash_count=0
[[ -f "${B}/old1/data.bin" ]] && (( local_trash_count++ )) || true
[[ -f "${B}/old2/data.bin" ]] && (( local_trash_count++ )) || true
# 移动后原路径应消失，trash 中应有一个
trash_count=$(ls "${B}/_trash_"*/old*/data.bin 2>/dev/null | wc -l | tr -d ' ')
if [[ "$trash_count" -eq 1 ]]; then
    pass "歧义场景：一个文件进 trash，一个被移动（贪心）"
else
    fail "歧义场景：trash 中应有 1 个，实际 ${trash_count} 个"
fi

# ── 测试 7：子目录递归 ────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 7：子目录递归同步 ==${NC}"
read -r A B <<< "$(setup t7)"
mkdir -p "${A}/a/b/c"
printf '%.0sx' {1..100} > "${A}/a/b/c/deep.txt"
run_sync "$A" "$B"
assert_file "${B}/a/b/c/deep.txt" "深层嵌套文件被复制"

# ── 测试 8：幂等性 ────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 8：幂等性（二次运行无操作）==${NC}"
read -r A B <<< "$(setup t8)"
mkdir -p "${A}/photos"
printf '%.0sx' {1..400} > "${A}/photos/img.jpg"
printf '%.0sx' {1..200} > "${A}/notes.txt"
run_sync "$A" "$B"
# 第二次运行，捕获输出检查"已同步"
output=$(echo "n" | "$BINARY" "$A" "$B" 2>&1)
if echo "$output" | grep -q "两个文件夹已完全同步"; then
    pass "二次运行输出：两个文件夹已完全同步"
else
    fail "二次运行应显示已同步，实际输出: $output"
fi

# ── 测试 9：trash 不被重复扫描 ───────────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 9：_trash_ 目录不参与同步逻辑 ==${NC}"
read -r A B <<< "$(setup t9)"
printf '%.0sx' {1..100} > "${A}/file.txt"
printf '%.0sx' {1..100} > "${B}/file.txt"
# 手动在 B 中创建一个 trash 目录（模拟历史 trash）
mkdir -p "${B}/_trash_20260101_000000"
printf '%.0sx' {1..999} > "${B}/_trash_20260101_000000/old.txt"
output=$(echo "n" | "$BINARY" "$A" "$B" 2>&1)
if echo "$output" | grep -q "两个文件夹已完全同步"; then
    pass "_trash_ 目录内容被忽略，不触发额外操作"
else
    fail "_trash_ 目录被错误扫描，输出: $output"
fi

# ── 测试 10：参数错误处理 ─────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}== 测试 10：参数错误处理 ==${NC}"
if ! "$BINARY" /nonexistent/path /tmp 2>/dev/null; then
    pass "源目录不存在时退出非零"
else
    fail "源目录不存在时应报错退出"
fi
if ! "$BINARY" /tmp /nonexistent/path 2>/dev/null; then
    pass "目标目录不存在时退出非零"
else
    fail "目标目录不存在时应报错退出"
fi
if ! "$BINARY" /tmp /tmp 2>/dev/null; then
    pass "A 和 B 相同时退出非零"
else
    fail "A 和 B 相同时应报错退出"
fi
if ! "$BINARY" 2>/dev/null; then
    pass "无参数时退出非零"
else
    fail "无参数时应报错退出"
fi

# ── 汇总 ──────────────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}══════════════════════════════${NC}"
total=$(( PASS + FAIL ))
if (( FAIL == 0 )); then
    echo -e "${GREEN}${BOLD}全部通过：${PASS}/${total}${NC}"
else
    echo -e "${RED}${BOLD}失败：${FAIL}/${total}${NC}"
    exit 1
fi

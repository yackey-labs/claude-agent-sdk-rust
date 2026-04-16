#!/usr/bin/env bash
# A/B benchmark: Python vs Rust claude-agent-sdk
# Runs each SDK N times, collects wall time / CPU / RSS, prints a summary.
set -euo pipefail
cd "$(dirname "$0")/.."

N=${1:-3}
echo "=== Claude Agent SDK A/B Benchmark (N=$N) ==="
echo ""

# ---- Build Rust bench binary ----
echo "Building Rust benchmark..."
cargo build --release --example quickstart 2>/dev/null
# Also build a dedicated bench binary
rustc_flags="-L target/release/deps"
cargo build --release 2>/dev/null

# Build the bench binary as a standalone binary linked to the lib
cat > /tmp/bench_rust_build.rs <<'RUSTEOF'
fn main() {}
RUSTEOF
# Simpler: add it as an example
cp bench/bench_rust.rs examples/bench_rust.rs
cargo build --release --example bench_rust 2>/dev/null

# ---- Setup Python venv ----
echo "Setting up Python environment..."
VENV="/tmp/claude-sdk-bench-venv"
if [ ! -d "$VENV" ]; then
    uv venv "$VENV" -q
fi
ACTIVATE="$VENV/bin/activate"
source "$ACTIVATE"
uv pip install claude-agent-sdk -q 2>/dev/null

echo ""
echo "Running $N iterations each..."
echo ""

# ---- Collect results ----
py_walls=()
py_rss=()
py_cpu=()
rs_walls=()
rs_rss=()
rs_cpu=()

parse_output() {
    # Parse key: value output lines into associative-like vars
    local line
    while IFS= read -r line; do
        case "$line" in
            wall_ms:*)    _wall="${line#wall_ms: }" ;;
            max_rss_kb:*) _rss="${line#max_rss_kb: }" ;;
            self_user_ms:*) _cpu="${line#self_user_ms: }" ;;
        esac
    done
}

for i in $(seq 1 "$N"); do
    echo "--- Run $i/$N ---"

    # Python
    _wall=0; _rss=0; _cpu=0
    output=$(python3 bench/bench_python.py 2>/dev/null)
    parse_output <<< "$output"
    py_walls+=("$_wall")
    py_rss+=("$_rss")
    py_cpu+=("$_cpu")
    echo "  Python: wall=${_wall}ms rss=${_rss}KB cpu=${_cpu}ms"

    # Rust
    _wall=0; _rss=0; _cpu=0
    output=$(./target/release/examples/bench_rust 2>/dev/null)
    parse_output <<< "$output"
    rs_walls+=("$_wall")
    rs_rss+=("$_rss")
    rs_cpu+=("$_cpu")
    echo "  Rust:   wall=${_wall}ms rss=${_rss}KB cpu=${_cpu}ms"
done

# ---- Compute averages ----
avg() {
    local sum=0
    local count=0
    for v in "$@"; do
        sum=$((sum + v))
        count=$((count + 1))
    done
    echo $((sum / count))
}

echo ""
echo "=== Results (avg over $N runs) ==="
echo ""
printf "%-12s %12s %12s %12s\n" "" "Wall (ms)" "RSS (KB)" "CPU user (ms)"
printf "%-12s %12s %12s %12s\n" "Python" "$(avg "${py_walls[@]}")" "$(avg "${py_rss[@]}")" "$(avg "${py_cpu[@]}")"
printf "%-12s %12s %12s %12s\n" "Rust" "$(avg "${rs_walls[@]}")" "$(avg "${rs_rss[@]}")" "$(avg "${rs_cpu[@]}")"

# Cleanup
rm -f examples/bench_rust.rs
deactivate 2>/dev/null || true

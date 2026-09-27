#!/usr/bin/env bash
# 重新生成许可页里 Rust 依赖的许可清单（assets/licenses/rust-crates.txt）。
# 依赖变了（Cargo.lock 有改动）就跑一次，结果随代码入库。
#
#   ./scripts/licenses.sh
#
# 需要 cargo-about：cargo install cargo-about --locked --features cli
# 配置见 about.toml（接受的许可、只算 iOS / macOS 目标、不列本仓库自己的 crate），
# 输出格式见 about.hbs（每段以「@@@ 包名 版本, …」开头，App 的许可页按它解析）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

# cargo 通常装在 ~/.cargo/bin，但非登录 shell 常常没有这个 PATH。
if [ -x "${HOME}/.cargo/bin/cargo" ]; then
  export PATH="${HOME}/.cargo/bin:${PATH}"
fi

cargo about generate \
  --manifest-path native/hub/Cargo.toml \
  -c about.toml \
  --fail \
  about.hbs \
  -o assets/licenses/rust-crates.txt

echo "wrote assets/licenses/rust-crates.txt ($(grep -c '^@@@' assets/licenses/rust-crates.txt) license texts)"

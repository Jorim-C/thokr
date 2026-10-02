#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"

./compile.sh
script -qec './executable --prompt ""' /dev/null
#!/usr/bin/env bash
# Generate Python gRPC stubs from runway_selector.proto into package/plugin/.
# Run once before packaging the area — the generated files are then committed
# to the tarball so end users do not need protoc / grpcio-tools.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
PROTO_DIR="$ROOT/../../runway_selector_protocol/proto"

cd "$ROOT/package"

# Ensure the runtime + tooling are available via mise.
mise install -q

mise exec python@3.12 -- python -m pip install --quiet \
    grpcio-tools \
    grpcio-health-checking

mise exec python@3.12 -- python -m grpc_tools.protoc \
    -I "$PROTO_DIR" \
    --python_out=plugin \
    --grpc_python_out=plugin \
    "$PROTO_DIR/runway_selector.proto"

echo "Generated plugin/runway_selector_pb2.py and plugin/runway_selector_pb2_grpc.py"

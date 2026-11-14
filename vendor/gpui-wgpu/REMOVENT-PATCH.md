# Local presentation fix

Based on crates.io gpui-pre-wgpu 0.3.8 (Apache-2.0).

Only the adapter sort order is changed: CPU devices on Linux prefer OpenGL.
Discrete/integrated/virtual GPUs and explicit device priorities retain upstream
ordering. In the Debian 12 arm64 Xvfb fixture, Mesa 22/LLVM 15 Vulkan accepted
surface creation and frame submission but presented a black content area.
The same scene and renderer produced the complete interface through OpenGL.
`scripts/qa/portable_app.py` requires varied pixels inside the content area and
exports a screenshot so a native window frame cannot satisfy the check alone.

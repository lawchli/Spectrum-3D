# 分析对齐夹具

`*.svf` 是小端二进制参考结果，给 Node、Rust 和 Kotlin 使用。每个文件含一秒、48 kHz、Float32 单声道 PCM，以及 60/120/240/480 行每秒 × 1/2/4/8 段每半音的 16 组逐行字节。五个文件共 80 组。

信号为 A4 +25¢ 正弦、指数扫频、固定种子的白噪声、确定性和弦、浏览器实际测试音频 2 的 OfflineAudioContext 渲染。`pad` 夹具包含原实现的双失谐锯齿、低通、LFO 和启动包络；`chord` 则是用于跨图形后端截图的无随机正弦和弦。

文件布局：4 字节 `SVF1`，三个 u32（采样率、PCM 个数、场景数），PCM f32 数组；每场景四个 u32（行率、每半音段数、行数、每行字节数），随后逐行紧密排列的 u8。

允许误差：Rust/Kotlin 输出相对于 JS 每个字节最多相差 1（0.5 dB）；原因是 FFT 运算顺序、数学库三角函数/对数和浮点舍入边界。频点落在量化边界时允许跨一档，禁止频带错位、漏行和无界误差。A4 +25¢ 的峰值插值要求在 1 音分内。功率历史均值与环回另有独立测试。

重新生成（先在 8765 启动 `web/` 预览）：

```sh
node scripts/pad-fixture.mjs
node scripts/analysis-fixtures.mjs --pad toolchains/test-pad.f32
node --test tests/analysis.test.mjs
cargo test --manifest-path rust/Cargo.toml -p spectrum-core --release
bash scripts/android-build.sh testDebugUnitTest
```

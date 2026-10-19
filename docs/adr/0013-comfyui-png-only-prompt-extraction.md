# 只从 ComfyUI 生成的 PNG 里提取提示词

需求最初面向「所有 AI 作图程序」，但调研发现各家载体互不相同（A1111 的 `parameters`、NovelAI 的 `Comment`、InvokeAI 的 `invokeai_metadata`、JPEG/WebP 的 EXIF UserComment……），全部实现对当前用户没有收益：他的出图工作流只有 ComfyUI，唯一的真实样本 `target/test-img.png` 也正是 ComfyUI 输出。决定：检测范围收窄为 **PNG 的 `prompt` / `workflow` 文本块**；识别到 ComfyUI 图就按节点图取正向 / 负向，其它图片格式与其它生成器一律当作普通图片，不检测、不提示。

**Consequences**：一张 A1111 出的 PNG 会被静默当成普通图片（已知并接受）；解析器只需覆盖 ComfyUI 的节点图——sampler（`KSampler` / `SamplerCustomAdvanced` 经 guider）→ 直通 conditioning 节点 → 文本来源（`CLIPTextEncode` / `CLIPTextEncodeSDXL` / `smZ Nodes` / `PrimitiveString` / `ShowText`），再加一层兜底扁平扫描；A1111 的「参数行 ≥3 对」式 bug 兼容、EXIF 解码、WebP/RIFF 全部不需要，实现量从约 400 行降到约 120 行。检测只读 PNG 文本块，不解码像素。识别到 ComfyUI 但没有可用的正向文本时（节点被 bypass、文本来自运行期 LLM），不改动当前内容，只在状态栏说明。

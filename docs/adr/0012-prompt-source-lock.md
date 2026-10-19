# 提示词来源锁：从图片读出的提示词永久只读

> 2026-10-20 注记：来源锁的范围定为**只锁内容**——正负提示词两段、图片、以及锁本身（`set_locked(false)` 仍报错）；**标题与删除放行**（`Store::rename` 不看锁，`Store::delete` 对 `prompt_source = 'image'` 放行，普通锁仍拦截）。原因：来源锁解不开，再禁删除就等于条目没有出口；标题只是标签，不属于「程序读出来的内容」。

ADR-0007 的锁是用户可点图标切换的开关；新需求要求「AI 图导入的提示词不能解锁修改」。决定：`examples` 表新增 `prompt_source TEXT NOT NULL DEFAULT 'manual'`（只有 `manual` / `image` 两个值），锁仍然只有一个 `locked` 布尔，永久性由来源**派生**：`prompt_source = 'image'` 时界面不提供解锁手势（侧栏图标与普通锁同字形、改用强调色，点击只弹一句状态栏提示、不改状态），`Store::set_locked(id, false)` 对这类行直接返回错误。写入来源锁时一律同时置 `locked = true`；归档把 `prompt_source` 写进 `data.json`。

**Consequences**：数据层只有一个锁字段，所以「这条为什么锁」要靠 `prompt_source` 回答，不能靠 `locked`；`prompt_source = 'image'` 配 `locked = false` 是非法组合，由写入路径保证、Store 层不额外拦截。锁是防误改的护栏，不是 DRM——用户仍可复制文本、新建示例再粘贴，这是刻意保留的出口（另见 ADR-0014）。

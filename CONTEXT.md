# Prompt Gallery

一个本地桌面工具：把「一张图片 + 正向提示词 + 负向提示词」作为一条记录存下来，方便随时复制提示词到 ComfyUI 使用。使用 Rust 编写，目标为 Windows，强调低资源占用。

应用显示名称与窗口标题为 **Prompt Gallery**（导出文件名前缀 `prompt-gallery-<时间戳>.zip`）。

## Language

**示例 (Example)**:
提示词图库中的一条记录：包含一个标题、一张图片、以及正向与负向两段提示词文本。是侧边栏列表的基本单位。
_Avoid_: 样本、条目、卡片、记录（sample, item, card, record）

**当前示例 (Current Example)**:
侧边栏中当前选中的那一条示例；图片框和提示词框展示它的内容，底部「保存 / 删除」按钮作用于它。
_Avoid_: 活动示例、选中示例（active example）

**标题栏 (Title Bar)**:
窗口最顶部 40pt 高的自绘条（系统标题栏已关闭，见 ADR-0009）：左侧是应用图标、名称与「数据管理 / 关于 / 外观」，右侧是最小化 / 最大化还原 / 关闭；按住空白处拖动窗口，双击最大化或还原。
_Avoid_: 顶栏、头部、工具栏（header, top bar, toolbar）

**Chrome 按钮 (Chrome Button)**:
标题栏里与界面同色的文字按钮（数据管理 / 关于 / 外观）：常态无底色，悬停才出现浅底，按下加深；用于打开菜单或弹窗，不提交表单。
_Avoid_: 菜单按钮、标签页、链接（menu button, tab, link）

**草稿 (Draft)**:
通过侧边栏「+新示例」新建、尚未保存过的示例；保存时才会落库为新示例并触发图片复制入库。
_Avoid_: 未命名示例、临时示例（unsaved example, temp）

**自动保存 (Autosave)**:
已落库示例的内容改动自动写库：提示词文本在停止输入 500ms 后写库，图片选入 / 拖入 / 移除立即写库；切换示例、失焦、关窗、导出、上锁前强制写库（见 ADR-0006）。草稿不自动入库，仍要点「保存为新示例」。
_Avoid_: 即时保存、自动落库（instant save）

**锁定 (Locked)**:
示例的一种只读状态：标题 / 图片 / 正负提示词 / 删除被禁用，但拖动排序、选中查看、复制提示词仍然可用（见 ADR-0007）。数据层以 `examples.locked` 强制，界面置灰只是表现。锁分两种：用户自己开关的普通锁，与程序加的来源锁（见下）。

**普通锁 (Manual Lock)**:
用户自己点侧边栏锁图标加上的锁：标题 / 图片 / 提示词 / 删除都被禁用，再点一下就能解开，所以不会把人锁死（见 ADR-0007）。
_Avoid_: 只读示例、冻结（read-only example, frozen）

**来源锁 (Source Lock)**:
提示词由程序从图片里读出来时自动加的锁：提示词两段、图片与锁本身都动不了，也解不开（见 ADR-0012）。**标题与删除不在锁的范围内**——条目照旧可以改名、可以删除，否则程序读进来的示例会既改不了名、也删不掉。界面上用强调色图标区分，点它只会弹一句说明。
_Avoid_: 永久锁、硬锁（hard lock, permanent lock）

**外观设置 (Appearance)**:
界面配色在「跟随系统 / 浅色 / 深色」之间的选择，存在数据目录的 `config.json`（`theme` 字段），下次启动沿用；只影响配色与控件视觉，不改变图库内容。
_Avoid_: 主题模式、皮肤、风格（theme mode, skin, style）

**正向提示词 (Positive Prompt)**:
示例的提示词正文，一段连续文本，内部不再分区（见 ADR-0011）；复制到剪贴板时原文照抄，供直接粘贴进 ComfyUI。
_Avoid_: 提示语、prompt 正文（prompt body）

**负向提示词 (Negative Prompt)**:
与正向提示词成对的一段排除性文本，说明画面里不要出现什么；单段、篇幅通常比正向短，可以留空。与正向提示词同权：一起自动保存、各自可复制、锁定时一起只读。
_Avoid_: 反向提示词、负面词（reverse prompt, unwanted prompt）

**数据库 (Database)**:
程序的持久化存储：一个 SQLite 文件（存示例元数据与提示词文本）+ 一个托管 `images/` 目录（存图片文件，以内容哈希命名、去重）。默认位于 `%APPDATA%\PromptGallery\`，可用环境变量 `PROMPT_GALLERY_DATA_DIR` 指到别处。
_Avoid_: 存储、仓库（storage, repository）

**托管图片 (Managed Image)**:
已经复制进托管 `images/` 目录、以内容哈希命名的图片文件；示例保存后，其图片即为托管图片。
_Avoid_: 图库图片、入库图片（stored image）

**待入库图片 (Pending Image)**:
草稿上已选择、但尚未复制进托管目录的图片；程序只记住源文件路径，点「保存」时才按内容哈希入库。放弃改动不会留下文件。（已落库示例选图即入库，不产生待入库图片，见 ADR-0006。）
_Avoid_: 临时图片、未保存图片（temp image）

**导出选择 (Export Selection)**:
导出图库时勾选、要打进 zip 的示例集合；打开弹窗时默认全选，可逐条取消或用「全选」在「全选 / 清空」之间切换。只决定当次导出包的内容，不改变图库本身。
_Avoid_: 筛选、过滤、批量选择（filter, batch select）

**新建行 (New Example Row)**:
固定在侧边栏列表最上方、始终可见、不可拖动的一行，显示 `+新示例`；不显示缩略图，点击新建一个草稿。与普通示例行同尺寸，但不参与拖动与重排落点计算。
_Avoid_: 加号按钮、新建按钮（plus button, new button）

**拖动虚影 (Drag Ghost)**:
长按拖动达标后跟随指针浮起的被拖行的整行副本（保留缩略图与标题）；被拖的原行原位留一个变淡的占位。
_Avoid_: 拖影、浮层、拖拽预览（drag preview, overlay）

**长按拖动 (Long-press Drag)**:
侧边栏重排示例的手势：在某行上按住不动约 0.28 秒后进入可拖动；达标前累计位移超过 6pt 即取消本次长按。
_Avoid_: 拖拽排序、按住拖动（drag sort, press-drag）

**测试钩子 (Test Hook)**:
应用内建的调试期实机驱动通道（见 ADR-0008）：环境变量 `PROMPT_GALLERY_TEST_HOOK` 指向一个目录后，应用从 `<目录>/requests.jsonl` 读指令、在 `raw_input_hook` 里注入真实输入事件、用 GPU 回读截图写到 `<目录>/shots/`、每步往 `<目录>/responses.jsonl` 写一条应答。仅 debug 构建存在，与窗口是否在前台 / 被遮挡无关；窗口最小化时没有帧。
_Avoid_: 测试模式、自动化接口、后门命令（test mode, automation API）

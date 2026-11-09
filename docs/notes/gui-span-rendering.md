# 原生 GUI 的文字区域横线调查

## 后续对照：不能继续归因于 span

移除生产 GUI 的全部 span 后，用户仍报告弹窗中存在横线，因此“换标签即可修复”的假设不成立。

检查 blitz-paint 0.2.1 发现 `draw_border_edge` 对透明颜色跳过绘制，但没有先按边宽过滤零宽边；这提供了另一个待检验方向，不足以直接证明截图根因。

使用临时离屏程序对比 HtmlDocument 与 DioxusDocument 0.7.10，按同版 blitz-dom / blitz-paint / Vello 渲染标签、输入框、下拉框近似结构，并测试默认边框色与透明边框色、1.0 与 1.25 缩放。已加载系统中文字体；对照中没有复现用户截图中的额外横线，透明边框也未产生可用于证明修复的差异。因此未在生产 GUI 加入全局透明边框补丁，也没有修改渲染依赖。

相关 [Blitz PR #902](https://github.com/DioxusLabs/blitz/pull/902) 是零宽边与分数缩放导致**边框消失**的修复提案，并非已确认修复多余横线；两者不能混同。后续应取得完整原生窗口截图、系统缩放比例和完整重启后的对照，区分真实布局边框、绘制伪影、热更新及窗口合成路径。

## 现象与边界

用户在 `dx serve --platform desktop --renderer native` 下观察到下拉框文字区域有淡色横线，并怀疑其他 `span` 也存在同样现象。

- 清除文字标签的 border / outline / text-decoration，以及将 overflow 从 hidden 改为 clip，未解决用户截图中的现象。
- 下拉框已改为不裁剪的 `div` 标签，长文本换行。
- 检查生产 GUI 剩余的 4 处 `span`：主页的账号/大区标签、设置行标签、下拉选项的勾选图标容器。它们没有主动设置下划线，已全部改为 `div` 以减少 inline 路径依赖。
- **以上是规避尝试，不是已确认修复。尚不能断言所有 span 都会画横线，也没有证据证明横线来自 CSS underline 或 scrollbar。**

本地锁定版本为 Dioxus / dioxus-native 0.7.10、blitz-dom 0.2.4、blitz-paint 0.2.1。

## 上游 issue 调查

查询了 Dioxus 和 Blitz 的公开 issue。未找到与“无下划线声明，文字区域却出现淡色横线”完全一致且确认修复的 issue。相关但不等价的已知问题：

- [Blitz #256 — Tracking: Layout and Text](https://github.com/DioxusLabs/blitz/issues/256)：跟踪 inline span 的 margin / border / padding 及 baseline 等布局能力；不代表本问题根因已定位。
- [Blitz #340 — Backgrounds are not drawn on inline elements](https://github.com/DioxusLabs/blitz/issues/340)：inline 元素背景缺失；[#352](https://github.com/DioxusLabs/blitz/issues/352) 为重复报告。该问题不是“默认下划线”。
- [Blitz #888 — text-overflow: ellipsis](https://github.com/DioxusLabs/blitz/issues/888)：指出 Stylo 可以解析属性，但 Blitz 当时没有对应省略号绘制；因此不能把浏览器里的省略样式当作原生端可靠能力。该问题也不证明横线由裁剪引起。

检查当前 blitz-paint 的 `src/text.rs`：装饰线绘制由 `text_decoration_line` 中的 UNDERLINE / LINE_THROUGH 标记控制，未见针对所有 span 默认加下划线的分支。仅靠静态源码检查不能解释截图。

## 独立对照

添加了 `packages/eorzea-app/examples/span_probe.rs`，不读取账号或配置、不发起网络请求：

```bash
cargo run -p eorzea-app --example span_probe
```

也可在该 crate 目录使用同一个示例比较两个渲染后端：

```bash
dx serve --example span_probe --platform desktop --renderer native
dx serve --example span_probe --platform web
```

示例依次显示普通 inline span、flex + span + hidden 裁剪、flex + div 无裁剪、button 外的 div。下一步需要在出现横线的原生环境确认哪些行受影响，并比较完整重启、热更新及浏览器的差异；如都受影响，再排查字体 / GPU 绘制，而不是继续将问题归因于 span。取得最小复现证据后再考虑报告上游；本次未创建 issue 或升级渲染依赖。

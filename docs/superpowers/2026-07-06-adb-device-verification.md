# adb 真机验证记录(2026-07-06)

设备:小米电视 MiTV_ANSP0(网络 adb,`192.168.x.x:5555`),Android 9 / SDK 28。此前 adb 相关实现均基于文档推断,本次为首次真机核对。

## 验证结论

| 项 | 结论 |
|---|---|
| `adb devices -l` 字段 | `serial state product: model: device: transport_id:` —— 与 `parse_adb_devices` 的解析字段吻合(`device:` 字段被忽略,预期内) |
| `-v threadtime` 输出格式 | 与解析器匹配;**发现 tag 为固定宽度空格填充**(如 `vold    :`),解析器原样保留尾部空格 → 已修复(`parse_threadtime_ref` 对 tag `trim_end`,commit f875fe9) |
| `logcat -T "MM-DD HH:MM:SS.mmm"` | **Android 9 可用**。语义为**包含**该时间戳(同毫秒的行会重复输出)——resume 续抓在同毫秒边界最多重复几行,可接受 |
| `-T` 非法时间串 | logcat 立即报错退出(`not in time format`)——正好落在代码预设的降级路径:流自动停止,用户重新 Start 全量抓取 |
| 多缓冲区 `-b main -b system` | 可用,输出按缓冲区分段(`--------- beginning of ...` 分隔行) |
| 缓冲区规格 | main/system/crash 各 4MB ring buffer(此设备无 radio/events 独立输出异常,未深测) |
| `adb track-devices` 协议 | 流式输出,4 位十六进制长度前缀 + 载荷(行格式 `serial\tstate\n`),如 `001b192.168.x.x:5555\tdevice\n` —— 为后续"设备监听长连接"(替代 4s 轮询)提供了确定的解析依据 |

## 遗留

- `-T` 的包含语义导致 resume 在同毫秒边界可能重复少量行;若要消除,需在续抓开始时跳过与上次末尾时间戳相同且内容重复的行(去重窗口),暂不做。
- 本记录仅覆盖 Android 9;更旧(≤6)与更新(≥13)版本的 `-T`/格式差异仍未验证。

## 规划:track-devices 设备监听长连接(2026-07-06 定为规划项,暂不实施)

用途:以订阅长连接替代当前 4s 轮询 `adb devices -l`,插拔感知毫秒级,消除周期性子进程开销。

已确认的技术事实:
- 协议:`adb track-devices` 输出为流式帧,4 位十六进制长度前缀 + 载荷,载荷每行 `serial\tstate`(真机抓包样本 `001b192.168.x.x:5555\tdevice\n`)。
- track-devices 只是本机 adb server(localhost:5037)的**只读订阅客户端**:不锁设备、不占设备通道、不监听端口,与用户本地的 adb / Android Studio 并存无干扰;与设备侧 5555 调试端口无关(该端口由 adb server 连接并多路复用)。

实施前需先定的决策(用户要求与"是否内置 platform-tools"一起考虑):
1. **内置 adb 的版本互杀问题**:内置 adb 与用户本机 adb 版本不一致时,后启动方会 kill 并重启 adb server,干扰用户其他工具。若内置,应遵循"优先复用系统 adb,找不到才用内置"的惯例。
2. 断线重连:用户 `adb kill-server` 或 server 崩溃后长连接中断,需要指数退避重连 + 重连期间回退轮询。
3. 首帧语义:连接建立即收到全量设备列表快照,后续为增量推送,状态机需区分。

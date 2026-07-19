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
- track-devices 长连接监听:协议已确认,实现待排期(需处理 adb server 重启后的重连)。
- 本记录仅覆盖 Android 9;更旧(≤6)与更新(≥13)版本的 `-T`/格式差异仍未验证。

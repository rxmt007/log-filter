# LogFilter Problems Android TV Demo

这是一个面向 Android TV / TV Box 的小型测试夹具，用真实 Android 平台行为验证
LogFilter Problems 面板。应用不尝试判断根因，也不会伪造系统日志文本。

## 遥控器交互

- 方向键：移动焦点。
- OK / Enter：触发按钮。
- Java 崩溃、Java OOM、ANR、native crash 和进程退出均需在 5 秒内按两次确认。
- 崩溃或进程退出后，从 TV Launcher 重新打开应用即可继续测试。

首页提供以下动作：

| 按钮 | 预期平台现象 | Problems 预期分类 |
| --- | --- | --- |
| 普通 E/F 日志（负对照） | 输出多条 `Log.e` 和一条 ASSERT/F 级文本，应用继续运行 | 不应产生故障事件 |
| Java/Kotlin 崩溃 | 主线程抛出未捕获异常 | Java/Kotlin |
| Java OOM | 应用进程持续分配 Java 堆直到未捕获 OOM | Java OOM |
| ANR | 前台显式广播的 Receiver 在主线程中阻塞 30 秒 | ANR |
| Native SIGSEGV | 向自身进程发送 `SIGSEGV` | Native / Signal |
| 进程退出 | 应用主动结束进程，重新打开后可观察生命周期 | 进程重启（取决于日志来源） |

LMK 和 Kernel OOM 不在 Demo 中模拟。它们需要系统或内核级内存压力，普通 APK
无法在不影响整机稳定性的前提下确定性触发。

## 构建

需要 Android SDK（包含 API 31）和 JDK 17：

```sh
export JAVA_HOME="$("/usr/libexec/java_home" -v 17)"
./gradlew :app:assembleDebug
```

APK 输出：

```text
app/build/outputs/apk/debug/app-debug.apk
```

TV Launcher banner 的可编辑源稿位于 `design/tv_banner.svg`，Android 打包使用
`app/src/main/res/drawable-xhdpi/tv_banner.png`（320×180）。

## 安装与启动

```sh
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start \
  -n com.example.logfilterproblemdemo/.MainActivity
```

卸载：

```sh
adb uninstall com.example.logfilterproblemdemo
```

建议先用同时包含 `main`、`system`、`crash` 和 `events` buffer 的命令开始抓取，
再启动 Demo 并逐项触发。触发后等待系统写入相关日志，最后停止抓取，让尾部尚未
闭合的多行事件完成定稿。只抓 `main` 时，部分事件可能没有足够来源信息，
Problems 会按设计保持为空而不是猜测。

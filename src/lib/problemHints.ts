import type { ProblemKind } from "@/types";

export const PROBLEM_KINDS = [
  "java-crash",
  "java-oom",
  "anr",
  "native-crash",
  "process-restart",
  "signal-exit",
  "lmk-kill",
  "kernel-oom-kill",
] as const satisfies readonly ProblemKind[];

const HINTS = {
  "java-crash": Object.freeze([
    "从首个业务栈帧开始核对触发路径与输入条件。",
    "比较同组事件的共同栈帧，并结合版本与操作步骤复现。",
  ]),
  "java-oom": Object.freeze([
    "结合事件前后的 GC、分配与内存统计，检查资源增长是否持续。",
    "确认异常发生在线程、对象类型与当时设备内存压力。",
  ]),
  anr: Object.freeze([
    "查看事件附近主线程、Binder 与锁等待信息。",
    "把系统记录的 Reason 作为调查入口，并结合线程状态验证。",
  ]),
  "native-crash": Object.freeze([
    "优先核对 signal、首个稳定 backtrace frame、模块与 BuildId。",
    "如有对应符号文件，在外部工具中完成符号化后再判断调用路径。",
  ]),
  "process-restart": Object.freeze([
    "比较结束与再次启动之间的系统事件，确认是否符合预期生命周期。",
    "检查重启前后进程身份、用户与启动原因是否一致。",
  ]),
  "signal-exit": Object.freeze([
    "核对系统记录的 signal 与同一进程实例附近的独立证据。",
    "检查是否存在配套 tombstone、崩溃记录或主动终止操作。",
  ]),
  "lmk-kill": Object.freeze([
    "查看被终止进程当时的 oom_adj、内存占用与设备整体压力。",
    "结合前台状态和内存回收策略，判断该终止是否符合系统策略。",
  ]),
  "kernel-oom-kill": Object.freeze([
    "区分 global 与 memcg 机制，并核对被终止进程所属内存域。",
    "结合事件前后的内核内存统计与约束信息继续调查。",
  ]),
} satisfies Record<ProblemKind, readonly string[]>;

export function problemHints(kind: ProblemKind): readonly string[] {
  return HINTS[kind];
}

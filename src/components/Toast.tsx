import { useEffect } from "react";
import { X } from "lucide-react";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastState {
  /** 唯一序号:变化即视为新 toast,重置自动消失计时器。 */
  id: number;
  message: string;
  tone?: "success" | "info" | "error";
  action?: ToastAction;
}

interface ToastProps {
  toast: ToastState | null;
  onDismiss: () => void;
}

const AUTO_DISMISS_MS = 6000;

/** 应用级底部居中提示条:导出完成/取消的全局信号(即使导出对话框已关闭也可见)。 */
export function Toast({ toast, onDismiss }: ToastProps) {
  // id 变化(新提示)或卸载时清理旧计时器,保证每条提示各自计时 6s。
  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(onDismiss, AUTO_DISMISS_MS);
    return () => window.clearTimeout(timer);
  }, [toast, onDismiss]);

  if (!toast) return null;

  return (
    <div className="lf-toast" data-tone={toast.tone ?? "info"} role="status" aria-live="polite">
      <span className="lf-toast-message">{toast.message}</span>
      {toast.action ? (
        <button className="lf-toast-action" type="button" onClick={toast.action.onClick}>
          {toast.action.label}
        </button>
      ) : null}
      <button className="lf-toast-close" type="button" title="关闭" onClick={onDismiss}>
        <X />
      </button>
    </div>
  );
}

import { AlertTriangle, KeyRound, Loader2, RefreshCw, Trash2 } from "lucide-react";
import { Modal } from "../../components/Modal";
import {
  buttonDangerSolid,
  buttonPrimary,
  buttonSecondary,
  spinnerClass,
} from "../../lib/ui";
import type { TenantSummary } from "../../types";

export type TenantCodeAction = "regenerate" | "revoke";

interface TenantCodeDialogProps {
  tenant: TenantSummary;
  action: TenantCodeAction;
  pending: boolean;
  onConfirm: () => void;
  onClose: () => void;
}

/** 平台租户码操作的统一确认弹窗，避免依赖浏览器原生 prompt/confirm。 */
export function TenantCodeDialog(props: TenantCodeDialogProps) {
  const revoking = props.action === "revoke";
  const replacing = !revoking && Boolean(props.tenant.code);
  const title = revoking
    ? "撤销租户码"
    : replacing
      ? "修改租户码"
      : "设置租户码";
  const noticeTitle = revoking
    ? "撤销后将不能继续注册"
    : replacing
      ? "当前租户码将立即失效"
      : "系统将自动生成租户码";
  const description = revoking
    ? `确认撤销租户“${props.tenant.name}”的租户码？已注册用户不受影响。`
    : replacing
      ? `系统将为租户“${props.tenant.name}”生成新的租户码，无需手动输入。`
      : `系统将为租户“${props.tenant.name}”生成可用于注册的租户码，无需手动输入。`;
  const pendingLabel = revoking ? "正在撤销" : "正在生成";
  const confirmLabel = revoking ? "确认撤销" : replacing ? "生成新租户码" : "生成租户码";
  const toneClass = revoking
    ? "border-red-200 bg-red-50 dark:border-red-900 dark:bg-red-950/50"
    : "border-indigo-200 bg-indigo-50 dark:border-indigo-900 dark:bg-indigo-950/40";
  const iconClass = revoking
    ? "bg-red-100 text-red-700 dark:bg-red-950 dark:text-red-300"
    : "bg-indigo-100 text-indigo-700 dark:bg-indigo-950 dark:text-indigo-300";
  const textClass = revoking
    ? "text-red-900 dark:text-red-200"
    : "text-indigo-950 dark:text-indigo-200";
  const descriptionClass = revoking
    ? "text-red-800/80 dark:text-red-300/80"
    : "text-indigo-900/75 dark:text-indigo-300/80";

  return (
    <Modal
      titleId="tenantCodeActionTitle"
      title={title}
      className="max-w-lg"
      role="alertdialog"
      ariaDescribedBy="tenantCodeActionDescription"
      closeDisabled={props.pending}
      onClose={props.onClose}
    >
      <div className="grid gap-5">
        <div className={`flex items-start gap-3 rounded-lg border p-4 ${toneClass}`}>
          <span
            className={`grid size-9 shrink-0 place-items-center rounded-full ${iconClass}`}
            aria-hidden="true"
          >
            {revoking ? <AlertTriangle size={20} /> : <KeyRound size={20} />}
          </span>
          <div>
            <strong className={`block text-sm font-semibold ${textClass}`}>{noticeTitle}</strong>
            <p
              id="tenantCodeActionDescription"
              className={`mt-1 text-sm leading-6 ${descriptionClass}`}
            >
              {description}
            </p>
          </div>
        </div>

        <div className="flex flex-col-reverse justify-end gap-2 sm:flex-row">
          <button
            type="button"
            className={buttonSecondary}
            onClick={props.onClose}
            disabled={props.pending}
            autoFocus
          >
            取消
          </button>
          <button
            type="button"
            className={revoking ? buttonDangerSolid : buttonPrimary}
            onClick={props.onConfirm}
            disabled={props.pending}
          >
            {props.pending ? (
              <Loader2 className={spinnerClass} size={18} />
            ) : revoking ? (
              <Trash2 size={18} />
            ) : (
              <RefreshCw size={18} />
            )}
            {props.pending ? pendingLabel : confirmLabel}
          </button>
        </div>
      </div>
    </Modal>
  );
}

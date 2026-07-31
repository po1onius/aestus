import { AlertTriangle, Loader2, Trash2 } from "lucide-react";
import { buttonDangerSolid, buttonSecondary, spinnerClass } from "../lib/ui";
import { Modal } from "./Modal";

interface ConfirmDialogProps {
  title: string;
  description: string;
  confirmLabel: string;
  pendingLabel: string;
  pending: boolean;
  onConfirm: () => void;
  onClose: () => void;
}

/**
 * 危险操作统一确认框。
 * 使用 alertdialog 语义并默认聚焦“取消”，降低键盘操作时误触不可逆动作的风险。
 */
export function ConfirmDialog(props: ConfirmDialogProps) {
  return (
    <Modal
      titleId="dangerConfirmationTitle"
      title={props.title}
      className="max-w-lg"
      role="alertdialog"
      ariaDescribedBy="dangerConfirmationDescription"
      closeDisabled={props.pending}
      onClose={props.onClose}
    >
      <div className="grid gap-5">
        <div className="flex items-start gap-3 rounded-lg border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950/50">
          <span className="grid size-9 shrink-0 place-items-center rounded-full bg-red-100 text-red-700 dark:bg-red-950 dark:text-red-300" aria-hidden="true">
            <AlertTriangle size={20} />
          </span>
          <div>
            <strong className="block text-sm font-semibold text-red-900 dark:text-red-200">此操作无法撤销</strong>
            <p id="dangerConfirmationDescription" className="mt-1 text-sm leading-6 text-red-800/80 dark:text-red-300/80">
              {props.description}
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
            className={buttonDangerSolid}
            onClick={props.onConfirm}
            disabled={props.pending}
          >
            {props.pending ? <Loader2 className={spinnerClass} size={18} /> : <Trash2 size={18} />}
            {props.pending ? props.pendingLabel : props.confirmLabel}
          </button>
        </div>
      </div>
    </Modal>
  );
}

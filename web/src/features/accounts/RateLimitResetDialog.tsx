import { AlertCircle, Loader2, RotateCcw } from "lucide-react";
import { Modal } from "../../components/Modal";
import { formatDateTime, formatOptionalDateTime } from "../../lib/format";
import { buttonPrimary, spinnerClass } from "../../lib/ui";
import type {
  GptAccount,
  RateLimitResetCredit,
  RateLimitResetCreditsResponse,
} from "../../types";

interface RateLimitResetDialogProps {
  account: GptAccount;
  response: RateLimitResetCreditsResponse | null;
  loading: boolean;
  error: string | null;
  applyingCreditId: string | null;
  onApply: (credit: RateLimitResetCredit) => void;
  onClose: () => void;
}

/**
 * GPT 额度重置弹窗展示上游返回的兑换列表，并将每次应用动作绑定到明确的 credit ID。
 * 列表不会根据 availableCount 人工补项；后端可能限制详情数量，计数与实际详情分别展示。
 */
export function RateLimitResetDialog(props: RateLimitResetDialogProps) {
  const accountLabel = props.account.email || props.account.account_id || props.account.id;
  const busy = props.loading || props.applyingCreditId !== null;

  return (
    <Modal
      titleId="rateLimitResetTitle"
      title="额度重置"
      description={`账号：${accountLabel}`}
      className="max-w-3xl"
      closeDisabled={busy}
      onClose={props.onClose}
    >
      <div className="grid gap-4">
        {props.error ? (
          <div className="flex items-start gap-3 rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-800 dark:border-red-900 dark:bg-red-950/40 dark:text-red-300">
            <AlertCircle className="mt-0.5 shrink-0" size={18} />
            <p className="min-w-0 break-words leading-6">{props.error}</p>
          </div>
        ) : props.loading && !props.response ? (
          <div className="flex min-h-40 items-center justify-center gap-2 rounded-lg border border-slate-200 text-sm text-slate-500 dark:border-slate-800 dark:text-slate-400">
            <Loader2 className={spinnerClass} size={20} />
            正在查询可用重置次数
          </div>
        ) : props.response?.credits.length ? (
          <ul className="grid gap-3" aria-label="可兑换额度重置列表">
            {props.response.credits.map((credit) => (
              <ResetCreditItem
                key={credit.id}
                credit={credit}
                applying={props.applyingCreditId === credit.id}
                disabled={busy}
                onApply={() => props.onApply(credit)}
              />
            ))}
          </ul>
        ) : (
          <div className="flex min-h-36 flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-slate-300 px-5 text-center dark:border-slate-700">
            <RotateCcw size={22} className="text-slate-400" />
            <strong className="text-sm font-semibold text-slate-700 dark:text-slate-300">
              当前没有可兑换的重置记录
            </strong>
            <p className="text-xs text-slate-500 dark:text-slate-400">
              上游返回的可用次数为 {props.response?.available_count ?? 0}。
            </p>
          </div>
        )}
      </div>
    </Modal>
  );
}

function ResetCreditItem(props: {
  credit: RateLimitResetCredit;
  applying: boolean;
  disabled: boolean;
  onApply: () => void;
}) {
  const available = props.credit.status === "available";

  return (
    <li className="flex flex-col gap-4 rounded-lg border border-slate-200 bg-white p-4 shadow-xs dark:border-slate-800 dark:bg-slate-950/40 sm:flex-row sm:items-center sm:justify-between">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <strong className="text-sm font-semibold text-slate-900 dark:text-slate-100">
            {props.credit.title || "Codex 额度重置"}
          </strong>
          <span className="rounded-full bg-slate-100 px-2 py-0.5 text-xs font-medium text-slate-600 dark:bg-slate-800 dark:text-slate-300">
            {resetStatusLabel(props.credit.status)}
          </span>
        </div>
        {props.credit.description && (
          <p className="mt-1.5 text-sm leading-6 text-slate-600 dark:text-slate-300">
            {props.credit.description}
          </p>
        )}
        <div className="mt-2 grid gap-1 text-xs leading-5 text-slate-500 dark:text-slate-400 sm:grid-cols-2 sm:gap-x-5">
          <span>获得：{formatDateTime(props.credit.granted_at)}</span>
          <span>过期：{formatOptionalDateTime(props.credit.expires_at)}</span>
          <span className="truncate sm:col-span-2" title={props.credit.id}>
            ID：{props.credit.id}
          </span>
        </div>
      </div>
      <button
        type="button"
        className={`${buttonPrimary} shrink-0`}
        disabled={props.disabled || !available}
        onClick={props.onApply}
        title={available ? "应用此额度重置" : "该重置记录当前不可应用"}
      >
        {props.applying ? (
          <Loader2 className={spinnerClass} size={16} />
        ) : (
          <RotateCcw size={16} />
        )}
        应用
      </button>
    </li>
  );
}

function resetStatusLabel(status: string) {
  switch (status) {
    case "available":
      return "可应用";
    case "redeeming":
      return "应用中";
    case "redeemed":
      return "已应用";
    default:
      return status;
  }
}

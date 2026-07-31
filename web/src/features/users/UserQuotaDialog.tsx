import { Loader2, Save } from "lucide-react";
import type { FormEvent } from "react";
import { Modal } from "../../components/Modal";
import { buttonPrimary, fieldLabel, fieldStack, inputClass, requiredMark, spinnerClass } from "../../lib/ui";
import type { DashboardUser } from "../../types";

interface UserQuotaDialogProps {
  user: DashboardUser;
  value: string;
  saving: boolean;
  onValueChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClose: () => void;
}

export function UserQuotaDialog(props: UserQuotaDialogProps) {
  return (
    <Modal
      titleId="userQuotaTitle"
      title="修改用户额度"
      description={`${props.user.username} · ${props.user.email}`}
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <div>
        <form className="grid gap-4" onSubmit={props.onSubmit}>
          <label className={fieldStack}>
            <span className={fieldLabel}>
              Token 额度<span className={requiredMark}>*</span>
            </span>
            <input
              className={inputClass}
              type="number"
              min="0"
              max={Number.MAX_SAFE_INTEGER}
              step="1"
              inputMode="numeric"
              value={props.value}
              onChange={(event) => props.onValueChange(event.target.value)}
              required
            />
          </label>
          <button
            className={`${buttonPrimary} mt-1 w-full`}
            disabled={props.saving || props.value.trim().length === 0}
          >
            {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
            保存额度
          </button>
        </form>
      </div>
    </Modal>
  );
}

import { Loader2, Save } from "lucide-react";
import { useState } from "react";
import { Modal } from "../../components/Modal";
import { buttonPrimary, fieldLabel, fieldStack, inputClass, requiredMark, spinnerClass } from "../../lib/ui";
import type { DashboardUser } from "../../types";

export interface UserQuotaDialogInput {
  value: string;
}

interface UserQuotaDialogProps {
  user: DashboardUser;
  saving: boolean;
  onSubmit: (input: UserQuotaDialogInput) => void;
  onClose: () => void;
}

export function UserQuotaDialog(props: UserQuotaDialogProps) {
  const [value, setValue] = useState<string>(() => props.user.quota.toString());
  return (
    <Modal
      titleId="userQuotaTitle"
      title="修改用户额度"
      description={`${props.user.username} · ${props.user.email}`}
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <div>
        <form className="grid gap-4" onSubmit={event => { event.preventDefault(); props.onSubmit({ value }); }}>
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
              value={value}
              onChange={(event) => setValue(event.target.value)}
              required
            />
          </label>
          <button
            className={`${buttonPrimary} mt-1 w-full`}
            disabled={props.saving || value.trim().length === 0}
          >
            {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
            保存额度
          </button>
        </form>
      </div>
    </Modal>
  );
}

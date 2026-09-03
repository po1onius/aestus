import { Loader2, Save } from "lucide-react";
import type { FormEvent } from "react";
import { Modal } from "../../components/Modal";
import {
  buttonPrimary,
  fieldHelp,
  fieldLabel,
  fieldStack,
  inputClass,
  spinnerClass,
} from "../../lib/ui";
import type { DashboardUser } from "../../types";

interface UserConcurrencyDialogProps {
  user: DashboardUser;
  value: string;
  maxValue: number;
  saving: boolean;
  onValueChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClose: () => void;
}

export function UserConcurrencyDialog(props: UserConcurrencyDialogProps) {
  return (
    <Modal
      titleId="userConcurrencyTitle"
      title="修改用户并发上限"
      description={`${props.user.username} · ${props.user.email}`}
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <form className="grid gap-4" onSubmit={props.onSubmit}>
        <label className={fieldStack}>
          <span className={fieldLabel}>每 Provider 最大并发数</span>
          <input
            className={inputClass}
            type="number"
            min="1"
            max={props.maxValue}
            step="1"
            inputMode="numeric"
            value={props.value}
            onChange={(event) => props.onValueChange(event.target.value)}
            placeholder="不限"
            disabled={props.saving}
            autoFocus
          />
          <span className={fieldHelp}>
            留空表示不限。上限按 Provider 分别计算，GPT 与 Claude 请求互不占用并发槽。
          </span>
        </label>
        <button className={`${buttonPrimary} mt-1 w-full`} disabled={props.saving}>
          {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
          保存并发上限
        </button>
      </form>
    </Modal>
  );
}

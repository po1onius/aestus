import { Building2, Loader2 } from "lucide-react";
import type { FormEvent } from "react";
import { Modal } from "../../components/Modal";
import {
  buttonPrimary,
  fieldLabel,
  fieldStack,
  inputClass,
  requiredMark,
  spinnerClass,
} from "../../lib/ui";

interface TenantCreateDialogProps {
  name: string;
  password: string;
  saving: boolean;
  onNameChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClose: () => void;
}

export function TenantCreateDialog(props: TenantCreateDialogProps) {
  const normalizedName = props.name.trim();
  const nameBytes = new TextEncoder().encode(normalizedName).length;
  const nameTooLong = nameBytes > 121;
  const passwordBytes = new TextEncoder().encode(props.password).length;
  const ownerRequested = props.password.length > 0;
  const passwordInvalid =
    ownerRequested && (Array.from(props.password).length < 8 || passwordBytes > 72);
  const normalizedOwnerName = normalizedName.toLowerCase();
  const ownerNameCharacters = Array.from(normalizedOwnerName);
  const ownerNameInvalid =
    ownerRequested &&
    (ownerNameCharacters.length < 5 ||
      ownerNameCharacters.length > 32 ||
      new TextEncoder().encode(normalizedOwnerName).length > 64 ||
      !/^[\p{L}\p{N}][\p{L}\p{N}_-]*$/u.test(normalizedOwnerName));

  return (
    <Modal
      titleId="tenantCreateTitle"
      title="添加租户"
      description="创建后，系统会自动生成用于注册的租户码。"
      className="max-w-lg"
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <form className="grid gap-4" onSubmit={props.onSubmit}>
        <label className={fieldStack}>
          <span className={fieldLabel}>
            租户名称<span className={requiredMark}>*</span>
          </span>
          <input
            className={inputClass}
            value={props.name}
            disabled={props.saving}
            maxLength={128}
            required
            autoFocus
            autoComplete="off"
            onChange={(event) => props.onNameChange(event.target.value)}
            placeholder="例如 AcmeCorp；创建 owner 时至少 5 个字符"
          />
        </label>

        <label className={fieldStack}>
          <span className={fieldLabel}>Owner 密码</span>
          <input
            className={inputClass}
            type="password"
            value={props.password}
            disabled={props.saving}
            autoComplete="new-password"
            onChange={(event) => props.onPasswordChange(event.target.value)}
            placeholder="至少 8 个字符；留空则暂不创建 owner"
          />
        </label>

        <button
          className={`${buttonPrimary} mt-1 w-full`}
          disabled={
            props.saving ||
            normalizedName.length === 0 ||
            nameTooLong ||
            passwordInvalid ||
            ownerNameInvalid
          }
        >
          {props.saving ? (
            <Loader2 className={spinnerClass} size={18} />
          ) : (
            <Building2 size={18} />
          )}
          {props.saving ? "正在添加" : "添加租户"}
        </button>
      </form>
    </Modal>
  );
}

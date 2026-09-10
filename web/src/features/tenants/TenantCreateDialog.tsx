import { Building2, Loader2 } from "lucide-react";
import { useState, type FormEvent } from "react";
import type { TenantLimits } from "../../types";
import { TenantLimitsFields, parseTenantLimits, tenantLimitsDraft } from "./TenantLimitsFields";
import { Modal } from "../../components/Modal";
import { PasswordInput } from "../../components/PasswordInput";
import {
  buttonPrimary,
  fieldLabel,
  fieldStack,
  inputClass,
  requiredMark,
  spinnerClass,
} from "../../lib/ui";

export interface CreateTenantInput {
  id: string;
  password: string | null;
  limits: TenantLimits;
}

interface TenantCreateDialogProps {
  saving: boolean;
  onCreate: (input: CreateTenantInput) => Promise<void>;
  onClose: () => void;
}

export function TenantCreateDialog(props: TenantCreateDialogProps) {
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [limitsDraft, setLimitsDraft] = useState(() => tenantLimitsDraft());
  const limits = parseTenantLimits(limitsDraft);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!limits || props.saving || (password && limits.max_users === 0)) return;
    await props.onCreate({ id: name.trim(), password: password || null, limits });
  }
  const normalizedName = name.trim();
  const nameBytes = new TextEncoder().encode(normalizedName).length;
  const nameTooLong = nameBytes > 121;
  const passwordBytes = new TextEncoder().encode(password).length;
  const ownerRequested = password.length > 0;
  const passwordInvalid =
    ownerRequested && (Array.from(password).length < 8 || passwordBytes > 72);
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
      <form className="grid gap-4" onSubmit={submit}>
        <label className={fieldStack}>
          <span className={fieldLabel}>
            租户名称<span className={requiredMark}>*</span>
          </span>
          <input
            className={inputClass}
            value={name}
            disabled={props.saving}
            maxLength={128}
            required
            autoFocus
            autoComplete="off"
            onChange={(event) => setName(event.target.value)}
            placeholder="例如 AcmeCorp；创建 owner 时至少 5 个字符"
          />
        </label>

        <div className={fieldStack}>
          <label className={fieldLabel} htmlFor="tenant-owner-password">Owner 密码</label>
          <PasswordInput
            id="tenant-owner-password"
            value={password}
            disabled={props.saving}
            autoComplete="new-password"
            onChange={(event) => setPassword(event.target.value)}
            placeholder="至少 8 个字符；留空则暂不创建 owner"
          />
        </div>

        <TenantLimitsFields value={limitsDraft} onChange={setLimitsDraft} disabled={props.saving} />

        <button
          className={`${buttonPrimary} mt-1 w-full`}
          disabled={
            props.saving ||
            !limits ||
            (ownerRequested && limits.max_users === 0) ||
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

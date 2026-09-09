import { Loader2, UserPlus } from "lucide-react";
import { useState } from "react";
import { Modal } from "../../components/Modal";
import {
  buttonPrimary,
  fieldHelp,
  fieldLabel,
  fieldStack,
  inputClass,
  requiredMark,
  spinnerClass,
} from "../../lib/ui";

export interface UserCreateDialogInput {
  username: string;
  email: string;
  password: string;
}

interface UserCreateDialogProps {
  saving: boolean;
  onSubmit: (input: UserCreateDialogInput) => void;
  onClose: () => void;
}

/** 管理员创建用户表单；邮箱默认值由服务端生成，这里只向管理员说明最终行为。 */
export function UserCreateDialog(props: UserCreateDialogProps) {
  const [username, setUsername] = useState<string>(() => "");
  const [email, setEmail] = useState<string>(() => "");
  const [password, setPassword] = useState<string>(() => "");
  const normalizedUsername = username.trim().toLowerCase();
  const defaultEmail = normalizedUsername ? `${normalizedUsername}@aes.tus` : "用户名@aes.tus";

  return (
    <Modal
      titleId="userCreateTitle"
      title="添加用户"
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <form className="grid gap-4" onSubmit={event => { event.preventDefault(); props.onSubmit({ username, email, password }); }}>
        <label className={fieldStack}>
          <span className={fieldLabel}>
            用户名<span className={requiredMark}>*</span>
          </span>
          <input
            className={inputClass}
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            placeholder="至少 5 个字符，例如 alice"
            autoComplete="off"
            maxLength={32}
            disabled={props.saving}
            required
            autoFocus
          />
        </label>

        <label className={fieldStack}>
          <span className={fieldLabel}>邮箱</span>
          <input
            className={inputClass}
            type="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            placeholder={defaultEmail}
            autoComplete="off"
            maxLength={254}
            disabled={props.saving}
          />
          <span className={fieldHelp}>留空时由服务端自动设置为 {defaultEmail}。</span>
        </label>

        <label className={fieldStack}>
          <span className={fieldLabel}>
            密码<span className={requiredMark}>*</span>
          </span>
          <input
            className={inputClass}
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            placeholder="至少 8 个字符"
            autoComplete="new-password"
            disabled={props.saving}
            required
          />
        </label>

        <button
          className={`${buttonPrimary} mt-1 w-full`}
          disabled={
            props.saving ||
            username.trim().length === 0 ||
            password.length === 0
          }
        >
          {props.saving ? (
            <Loader2 className={spinnerClass} size={18} />
          ) : (
            <UserPlus size={18} />
          )}
          {props.saving ? "正在添加" : "添加用户"}
        </button>
      </form>
    </Modal>
  );
}

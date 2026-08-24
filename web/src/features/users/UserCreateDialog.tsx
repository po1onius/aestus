import { Loader2, UserPlus } from "lucide-react";
import type { FormEvent } from "react";
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

interface UserCreateDialogProps {
  username: string;
  email: string;
  password: string;
  saving: boolean;
  onUsernameChange: (value: string) => void;
  onEmailChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClose: () => void;
}

/** 管理员创建用户表单；邮箱默认值由服务端生成，这里只向管理员说明最终行为。 */
export function UserCreateDialog(props: UserCreateDialogProps) {
  const normalizedUsername = props.username.trim().toLowerCase();
  const defaultEmail = normalizedUsername ? `${normalizedUsername}@aes.tus` : "用户名@aes.tus";

  return (
    <Modal
      titleId="userCreateTitle"
      title="添加用户"
      description="创建一个可直接登录 Dashboard 的普通用户。"
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <form className="grid gap-4" onSubmit={props.onSubmit}>
        <label className={fieldStack}>
          <span className={fieldLabel}>
            用户名<span className={requiredMark}>*</span>
          </span>
          <input
            className={inputClass}
            value={props.username}
            onChange={(event) => props.onUsernameChange(event.target.value)}
            placeholder="例如 alice"
            autoComplete="off"
            maxLength={32}
            disabled={props.saving}
            required
            autoFocus
          />
          <span className={fieldHelp}>最多 32 个字符，可使用字母、数字、下划线和连字符。</span>
        </label>

        <label className={fieldStack}>
          <span className={fieldLabel}>邮箱</span>
          <input
            className={inputClass}
            type="email"
            value={props.email}
            onChange={(event) => props.onEmailChange(event.target.value)}
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
            value={props.password}
            onChange={(event) => props.onPasswordChange(event.target.value)}
            placeholder="至少 8 个字符"
            autoComplete="new-password"
            disabled={props.saving}
            required
          />
          <span className={fieldHelp}>至少 8 个字符，UTF-8 编码长度不能超过 72 字节。</span>
        </label>

        <button
          className={`${buttonPrimary} mt-1 w-full`}
          disabled={
            props.saving ||
            props.username.trim().length === 0 ||
            props.password.length === 0
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

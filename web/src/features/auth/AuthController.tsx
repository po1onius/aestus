import { FormEvent, useState } from "react";
import { toast } from "sonner";
import { isDashboardAuthError } from "../../api/client";
import { useRequestScope } from "../../api/useRequestScope";
import { authPath } from "../../config";
import { AuthScreen } from "../../features/auth/AuthScreen";
import { showErrorToast } from "../../lib/errors";
import { utf8ByteLength } from "../../lib/validation";
import type { AuthResponse, DashboardTheme } from "../../types";
export function AuthController({ theme, loading, applyAuth }: { theme: DashboardTheme; loading: boolean; applyAuth: (data: AuthResponse) => void }) {
  const { requestJson } = useRequestScope(null);
  const [authSubmitting, setAuthSubmitting] = useState(false);

  const [authMode, setAuthMode] = useState<"login" | "register">("login");

  const [loginIdentifier, setLoginIdentifier] = useState("");

  const [loginPassword, setLoginPassword] = useState("");

  const [registerUsername, setRegisterUsername] = useState("");

  const [registerTenantCode, setRegisterTenantCode] = useState("");

  const [registerEmail, setRegisterEmail] = useState("");

  const [registerPassword, setRegisterPassword] = useState("");

  const [registerCode, setRegisterCode] = useState("");

  const [emailCodeSending, setEmailCodeSending] = useState(false);

  async function submitLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (utf8ByteLength(loginPassword) > 72) {
      toast.error("登录失败", { description: "密码 UTF-8 编码长度不能超过 72 字节。" });
      return;
    }
    setAuthSubmitting(true);
    try {
      const data = await requestJson<AuthResponse>(`${authPath}/login`, {
        method: "POST",
        body: JSON.stringify({
          identifier: loginIdentifier.trim(),
          password: loginPassword,
        }),
      });
      applyAuth(data);
      toast.success("登录成功");
    } catch (error) {
      if (isDashboardAuthError(error)) {
        // 登录接口没有携带既有 Dashboard token，因此不会触发全局会话失效 Toast；这里
        // 单独展示统一凭证错误，同时不区分邮箱不存在、密码错误或用户被禁用。
        console.error("[dashboard] 登录凭证校验失败", error);
        toast.error("登录失败", { description: "邮箱、用户名或密码错误。" });
      } else {
        showErrorToast("登录失败", error);
      }
    } finally {
      setAuthSubmitting(false);
    }
  }

  async function sendRegisterEmailCode() {
    setEmailCodeSending(true);
    try {
      await requestJson<{ status: string }>(`${authPath}/register/email-code`, {
        method: "POST",
        body: JSON.stringify({ email: registerEmail.trim() }),
      });
      toast.success("验证码已发送");
    } catch (error) {
      showErrorToast("验证码发送失败", error);
    } finally {
      setEmailCodeSending(false);
    }
  }

  async function submitRegister(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const username = registerUsername.trim().toLowerCase();
    const usernameCharacters = Array.from(username);
    if (
      usernameCharacters.length < 5 ||
      usernameCharacters.length > 32 ||
      utf8ByteLength(username) > 128 ||
      !/^[\p{L}\p{N}][\p{L}\p{N}_-]*$/u.test(username)
    ) {
      toast.error("注册失败", {
        description: "用户名必须为 5 到 32 个字符，只能包含字母、数字、下划线和连字符。",
      });
      return;
    }
    if (Array.from(registerPassword).length < 8 || utf8ByteLength(registerPassword) > 72) {
      toast.error("注册失败", {
        description: "密码至少 8 个字符，且 UTF-8 编码长度不能超过 72 字节。",
      });
      return;
    }
    setAuthSubmitting(true);
    try {
      const data = await requestJson<AuthResponse>(`${authPath}/register`, {
        method: "POST",
        body: JSON.stringify({
          username,
          tenant_code: registerTenantCode.trim(),
          email: registerEmail.trim(),
          password: registerPassword,
          email_code: registerCode.trim(),
        }),
      });
      applyAuth(data);
      toast.success("注册成功");
    } catch (error) {
      showErrorToast("注册失败", error);
    } finally {
      setAuthSubmitting(false);
    }
  }
  return <AuthScreen
    theme={theme}
    loading={loading}
    mode={authMode}
    submitting={authSubmitting}
    emailCodeSending={emailCodeSending}
    loginIdentifier={loginIdentifier}
    loginPassword={loginPassword}
    registerUsername={registerUsername}
    registerTenantCode={registerTenantCode}
    registerEmail={registerEmail}
    registerPassword={registerPassword}
    registerCode={registerCode}
    onModeChange={setAuthMode}
    onLoginIdentifierChange={setLoginIdentifier}
    onLoginPasswordChange={setLoginPassword}
    onRegisterUsernameChange={setRegisterUsername}
    onRegisterTenantCodeChange={setRegisterTenantCode}
    onRegisterEmailChange={setRegisterEmail}
    onRegisterPasswordChange={setRegisterPassword}
    onRegisterCodeChange={setRegisterCode}
    onLogin={submitLogin}
    onRegister={submitRegister}
    onSendEmailCode={sendRegisterEmailCode}
  />;
}

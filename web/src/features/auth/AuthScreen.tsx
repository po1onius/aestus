import { KeyRound, Loader2, Mail, Save } from "lucide-react";
import type { FormEvent } from "react";
import { Toaster } from "sonner";
import tokenGatewayLogo from "../../assets/token-gateway-logo.svg";
import { SlidingTabList } from "../../components/SlidingTabList";
import {
  buttonPrimary,
  buttonSecondary,
  cx,
  fieldHelp,
  fieldLabel,
  fieldStack,
  inputClass,
  spinnerClass,
  tabClass,
  tabContentClass,
  tabIdleClass,
  tabSelectedClass,
} from "../../lib/ui";
import type { DashboardTheme } from "../../types";

interface AuthScreenProps {
  theme: DashboardTheme;
  loading: boolean;
  mode: "login" | "register";
  submitting: boolean;
  emailCodeSending: boolean;
  loginIdentifier: string;
  loginPassword: string;
  registerUsername: string;
  registerTenantCode: string;
  registerEmail: string;
  registerPassword: string;
  registerCode: string;
  onModeChange: (mode: "login" | "register") => void;
  onLoginIdentifierChange: (value: string) => void;
  onLoginPasswordChange: (value: string) => void;
  onRegisterUsernameChange: (value: string) => void;
  onRegisterTenantCodeChange: (value: string) => void;
  onRegisterEmailChange: (value: string) => void;
  onRegisterPasswordChange: (value: string) => void;
  onRegisterCodeChange: (value: string) => void;
  onLogin: (event: FormEvent<HTMLFormElement>) => void;
  onRegister: (event: FormEvent<HTMLFormElement>) => void;
  onSendEmailCode: () => void;
}

/*
 * 使用动态视口高度避免浏览器工具栏改变可视区域后产生纵向偏移。
 * 卡片在几何中心上方保留少量视觉补偿，使表单主体而非阴影边界落在视觉中心。
 */
const authViewportClass =
  "flex min-h-dvh items-center justify-center bg-slate-50 p-4 text-slate-950 dark:bg-slate-950 dark:text-slate-100 sm:p-6";
const authPanelPositionClass = "-translate-y-4 sm:-translate-y-6";

export function AuthScreen(props: AuthScreenProps) {
  if (props.loading) {
    return (
      <main className={authViewportClass}>
        <Toaster position="top-right" theme={props.theme} richColors closeButton />
        <div className={cx("flex w-full max-w-sm items-center justify-center gap-3 rounded-xl border border-slate-200/80 bg-white/95 p-8 text-sm text-slate-600 shadow-[inset_0_1px_0_rgba(255,255,255,0.95),0_1px_2px_rgba(15,23,42,0.05),0_14px_32px_-18px_rgba(15,23,42,0.28)] backdrop-blur-sm dark:border-slate-800 dark:bg-slate-900/95 dark:text-slate-300 dark:shadow-[inset_0_1px_0_rgba(255,255,255,0.04),0_1px_2px_rgba(0,0,0,0.35),0_16px_36px_-18px_rgba(0,0,0,0.85)]", authPanelPositionClass)}>
          <Loader2 className={spinnerClass} size={22} />
          <span>正在检查登录状态</span>
        </div>
      </main>
    );
  }

  return (
    <main className={authViewportClass}>
      <Toaster position="top-right" theme={props.theme} richColors closeButton />
      <section className={cx("grid w-full max-w-md gap-6 rounded-2xl border border-slate-200/80 bg-white/95 p-6 shadow-[inset_0_1px_0_rgba(255,255,255,0.95),0_1px_2px_rgba(15,23,42,0.05),0_18px_42px_-20px_rgba(15,23,42,0.32)] backdrop-blur-sm dark:border-slate-800 dark:bg-slate-900/95 dark:shadow-[inset_0_1px_0_rgba(255,255,255,0.04),0_1px_2px_rgba(0,0,0,0.35),0_18px_42px_-20px_rgba(0,0,0,0.9)] sm:p-8", authPanelPositionClass)}>
        <div className="flex items-center gap-3">
          <img className="size-10 shrink-0" src={tokenGatewayLogo} alt="" aria-hidden="true" />
          <div>
            <h1 className="text-lg font-semibold tracking-tight text-slate-950 dark:text-slate-100">Aestus</h1>
          </div>
        </div>
        <SlidingTabList
          count={2}
          selectedIndex={props.mode === "login" ? 0 : 1}
          ariaLabel="登录注册切换"
        >
          <button
            className={cx(tabClass, props.mode === "login" ? tabSelectedClass : tabIdleClass)}
            type="button"
            onClick={() => props.onModeChange("login")}
            role="tab"
            aria-selected={props.mode === "login"}
          >
            <span className={tabContentClass}>登录</span>
          </button>
          <button
            className={cx(tabClass, props.mode === "register" ? tabSelectedClass : tabIdleClass)}
            type="button"
            onClick={() => props.onModeChange("register")}
            role="tab"
            aria-selected={props.mode === "register"}
          >
            <span className={tabContentClass}>注册</span>
          </button>
        </SlidingTabList>
        {props.mode === "login" ? (
          <form className="grid gap-4" onSubmit={props.onLogin}>
            <label className={fieldStack}>
              <span className={fieldLabel}>邮箱或用户名</span>
              <input
                className={inputClass}
                value={props.loginIdentifier}
                onChange={(event) => props.onLoginIdentifierChange(event.target.value)}
                autoComplete="username"
                maxLength={254}
                required
              />
            </label>
            <label className={fieldStack}>
              <span className={fieldLabel}>密码</span>
              <input
                className={inputClass}
                type="password"
                value={props.loginPassword}
                onChange={(event) => props.onLoginPasswordChange(event.target.value)}
                autoComplete="current-password"
                maxLength={72}
                required
              />
            </label>
            <button className={`${buttonPrimary} mt-1 w-full`} disabled={props.submitting}>
              {props.submitting ? <Loader2 className={spinnerClass} size={18} /> : <KeyRound size={18} />}
              登录
            </button>
          </form>
        ) : (
          <form className="grid gap-4" onSubmit={props.onRegister}>
            <label className={fieldStack}>
              <span className={fieldLabel}>租户码</span>
              <input
                className={inputClass}
                value={props.registerTenantCode}
                onChange={(event) => props.onRegisterTenantCodeChange(event.target.value)}
                autoComplete="organization"
                maxLength={128}
                required
              />
              <p className={fieldHelp}>租户内首位注册用户将成为 owner，后续注册用户为普通成员。</p>
            </label>
            <label className={fieldStack}>
              <span className={fieldLabel}>用户名</span>
              <input
                className={inputClass}
                value={props.registerUsername}
                onChange={(event) => props.onRegisterUsernameChange(event.target.value)}
                autoComplete="username"
                maxLength={128}
                required
              />
              <p className={fieldHelp}>最多 32 个字符，可使用字母、数字、下划线或连字符，注册后不可修改。</p>
            </label>
            <label className={fieldStack}>
              <span className={fieldLabel}>邮箱</span>
              <input
                className={inputClass}
                type="email"
                value={props.registerEmail}
                onChange={(event) => props.onRegisterEmailChange(event.target.value)}
                autoComplete="email"
                maxLength={254}
                required
              />
            </label>
            <label className={fieldStack}>
              <span className={fieldLabel}>密码</span>
              <input
                className={inputClass}
                type="password"
                value={props.registerPassword}
                onChange={(event) => props.onRegisterPasswordChange(event.target.value)}
                autoComplete="new-password"
                minLength={8}
                maxLength={72}
                required
              />
              <p className={fieldHelp}>至少 8 个字符，UTF-8 编码后最多 72 字节。</p>
            </label>
            <label className={fieldStack}>
              <span className={fieldLabel}>邮箱验证码</span>
              <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2">
                <input
                  className={inputClass}
                  value={props.registerCode}
                  onChange={(event) => props.onRegisterCodeChange(event.target.value)}
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  minLength={6}
                  maxLength={6}
                  pattern="[0-9]{6}"
                  required
                />
                <button
                  type="button"
                  className={buttonSecondary}
                  disabled={props.emailCodeSending || props.registerEmail.trim().length === 0}
                  onClick={props.onSendEmailCode}
                >
                  {props.emailCodeSending ? (
                    <Loader2 className={spinnerClass} size={18} />
                  ) : (
                    <Mail size={18} />
                  )}
                  发送
                </button>
              </div>
            </label>
            <button className={`${buttonPrimary} mt-1 w-full`} disabled={props.submitting}>
              {props.submitting ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
              注册
            </button>
          </form>
        )}
      </section>
    </main>
  );
}

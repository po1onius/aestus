import { ClipboardCheck, ExternalLink, Loader2, Play, Save } from "lucide-react";
import type { FormEvent } from "react";
import { Modal } from "../../components/Modal";
import { SlidingTabList } from "../../components/SlidingTabList";
import { defaultGptClientId } from "../../config";
import { formatDateTime } from "../../lib/format";
import {
  buttonPrimary,
  buttonSecondary,
  cx,
  fieldHelp,
  fieldLabel,
  fieldStack,
  inputClass,
  requiredMark,
  spinnerClass,
  tabClass,
  tabContentClass,
  tabIdleClass,
  tabSelectedClass,
  textareaClass,
} from "../../lib/ui";
import type { AccountImportMode, AccountProviderKey, OauthAuthorizationResponse } from "../../types";

interface AccountImportDialogProps {
  provider: AccountProviderKey;
  providerLabel: string;
  mode: AccountImportMode;
  authorization: OauthAuthorizationResponse | null;
  callbackUrl: string;
  refreshToken: string;
  clientId: string;
  chatgptAccountId: string;
  saving: boolean;
  oauthLoading: boolean;
  onClose: () => void;
  onModeChange: (mode: AccountImportMode) => void;
  onCreateAuthorization: () => void;
  onCopyAuthorizationUrl: () => void;
  onCallbackUrlChange: (value: string) => void;
  onRefreshTokenChange: (value: string) => void;
  onClientIdChange: (value: string) => void;
  onChatgptAccountIdChange: (value: string) => void;
  onSubmitCallback: (event: FormEvent<HTMLFormElement>) => void;
  onSubmitManual: (event: FormEvent<HTMLFormElement>) => void;
}

export function AccountImportDialog(props: AccountImportDialogProps) {
  const isClaude = props.provider === "claude";
  return (
    <Modal
      titleId="accountImportTitle"
      title={`添加 ${props.providerLabel} 账号`}
      description={isClaude ? "通过 Anthropic OAuth 导入 Max、Pro、Team 或 Enterprise 付费账号。" : undefined}
      closeDisabled={props.saving || props.oauthLoading}
      onClose={props.onClose}
    >
      {props.provider === "gpt" && (
        <SlidingTabList
          count={2}
          selectedIndex={props.mode === "oauth" ? 0 : 1}
          ariaLabel="账号导入方式"
        >
          <button
            className={cx(tabClass, props.mode === "oauth" ? tabSelectedClass : tabIdleClass)}
            type="button"
            onClick={() => props.onModeChange("oauth")}
            role="tab"
            aria-selected={props.mode === "oauth"}
          >
            <span className={tabContentClass}>OAuth 导入</span>
          </button>
          <button
            className={cx(tabClass, props.mode === "refreshToken" ? tabSelectedClass : tabIdleClass)}
            type="button"
            onClick={() => props.onModeChange("refreshToken")}
            role="tab"
            aria-selected={props.mode === "refreshToken"}
          >
            <span className={tabContentClass}>RT 导入</span>
          </button>
        </SlidingTabList>
      )}

      {props.mode === "oauth" ? (
        <div>
          <div className="grid gap-4">
            <button
              className={`${buttonPrimary} w-full`}
              type="button"
              onClick={props.onCreateAuthorization}
              disabled={props.oauthLoading || props.saving}
            >
              {props.oauthLoading ? <Loader2 className={spinnerClass} size={18} /> : <Play size={18} />}
              生成授权链接
            </button>
            {props.authorization && (
              <div className="grid gap-4 rounded-xl border border-slate-200 bg-slate-50 p-4 dark:border-slate-800 dark:bg-slate-950/60">
                <div className={fieldStack}>
                  <label className={fieldLabel}>授权链接</label>
                  <textarea className={`${textareaClass} bg-white font-mono text-xs dark:bg-slate-950`} readOnly value={props.authorization.authorization_url} rows={4} />
                </div>
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    className={buttonSecondary}
                    onClick={props.onCopyAuthorizationUrl}
                  >
                    <ClipboardCheck size={18} />
                    复制
                  </button>
                  <a
                    className={buttonSecondary}
                    href={props.authorization.authorization_url}
                    target="_blank"
                    rel="noreferrer"
                  >
                    <ExternalLink size={18} />
                    打开
                  </a>
                </div>
                <div className="grid gap-2 rounded-lg border border-slate-200 bg-white p-3 text-sm text-slate-600 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300" aria-label="OAuth 操作步骤">
                  <div className="grid grid-cols-[1.5rem_minmax(0,1fr)] items-start gap-2">
                    <strong className="grid size-6 place-items-center rounded-full bg-indigo-50 text-xs text-indigo-800 dark:bg-indigo-950/70 dark:text-indigo-300">1</strong>
                    <span>在浏览器完成登录授权。</span>
                  </div>
                  <div className="grid grid-cols-[1.5rem_minmax(0,1fr)] items-start gap-2">
                    <strong className="grid size-6 place-items-center rounded-full bg-indigo-50 text-xs text-indigo-800 dark:bg-indigo-950/70 dark:text-indigo-300">2</strong>
                    {isClaude ? (
                      <span>授权页会显示一次性的 authorization code。</span>
                    ) : (
                      <span>
                        授权后会跳转到 <code className="rounded bg-slate-100 px-1 py-0.5 font-mono text-xs text-slate-800 dark:bg-slate-800 dark:text-slate-200">{props.authorization.redirect_uri}</code>
                        ，页面无法加载是正常现象。
                      </span>
                    )}
                  </div>
                  <div className="grid grid-cols-[1.5rem_minmax(0,1fr)] items-start gap-2">
                    <strong className="grid size-6 place-items-center rounded-full bg-indigo-50 text-xs text-indigo-800 dark:bg-indigo-950/70 dark:text-indigo-300">3</strong>
                    <span>
                      {isClaude
                        ? "复制页面显示的 code#state，完整粘贴到下方。"
                        : "从浏览器地址栏复制包含 code 和 state 的完整 URL，粘贴到下方。"}
                    </span>
                  </div>
                </div>
                <p className={fieldHelp}>
                  过期时间：{formatDateTime(props.authorization.expires_at)}
                </p>
              </div>
            )}
            <form className={fieldStack} onSubmit={props.onSubmitCallback}>
              <label className={fieldLabel} htmlFor="callbackUrl">
                {isClaude ? "Authorization Result" : "Callback URL"}
              </label>
              <textarea
                className={textareaClass}
                id="callbackUrl"
                value={props.callbackUrl}
                onChange={(event) => props.onCallbackUrlChange(event.target.value)}
                maxLength={16 * 1024}
                rows={4}
                placeholder={
                  isClaude
                    ? "粘贴授权页显示的 code#state"
                    : "http://localhost:1455/auth/callback?code=...&state=..."
                }
              />
              <button
                className={`${buttonPrimary} mt-1 w-full`}
                disabled={props.saving || props.callbackUrl.trim().length === 0}
              >
                {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
                确认
              </button>
            </form>
          </div>
        </div>
      ) : (
        <div>
          <form className="grid gap-4" onSubmit={props.onSubmitManual}>
            <label className={fieldStack}>
              <span className={fieldLabel}>Client ID</span>
              <input
                className={inputClass}
                value={props.clientId}
                onChange={(event) => props.onClientIdChange(event.target.value)}
                placeholder={defaultGptClientId}
                autoComplete="off"
                maxLength={512}
              />
            </label>
            <label className={fieldStack}>
              <span className={fieldLabel}>ChatGPT Account ID</span>
              <input
                className={inputClass}
                value={props.chatgptAccountId}
                onChange={(event) => props.onChatgptAccountIdChange(event.target.value)}
                placeholder="可选，手动指定 chatgpt_account_id"
                autoComplete="off"
                maxLength={512}
              />
            </label>
            <label className={fieldStack}>
              <span className={fieldLabel}>
                Refresh Token<span className={requiredMark}>*</span>
              </span>
              <textarea
                className={textareaClass}
                value={props.refreshToken}
                onChange={(event) => props.onRefreshTokenChange(event.target.value)}
                rows={5}
                placeholder="粘贴 refresh_token"
                maxLength={32 * 1024}
                required
              />
            </label>
            <button
              className={`${buttonPrimary} mt-1 w-full`}
              disabled={props.saving || props.refreshToken.trim().length === 0}
            >
              {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
              保存账号
            </button>
          </form>
        </div>
      )}
    </Modal>
  );
}

import { ClipboardCheck, ExternalLink, Loader2, Play, Save } from "lucide-react";
import { useEffect, useState } from "react";
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
  initialMode: AccountImportMode;
  authorization: OauthAuthorizationResponse | null;
  saving: boolean;
  oauthLoading: boolean;
  onClose: () => void;
  onCreateAuthorization: () => void;
  onCopyAuthorizationUrl: () => void;
  onSubmitCallback: (callbackUrl: string) => void;
  onSubmitManual: (input: { refreshToken: string; clientId: string; chatgptAccountId: string }) => void;
}

export function AccountImportDialog(props: AccountImportDialogProps) {
  const [mode, setMode] = useState(props.initialMode);
  const [callbackUrl, setCallbackUrl] = useState("");
  const [refreshToken, setRefreshToken] = useState("");
  const [clientId, setClientId] = useState(defaultGptClientId);
  const [chatgptAccountId, setChatgptAccountId] = useState("");
  useEffect(() => { setCallbackUrl(""); }, [props.authorization]);
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
          selectedIndex={mode === "oauth" ? 0 : 1}
          ariaLabel="账号导入方式"
        >
          <button
            className={cx(tabClass, mode === "oauth" ? tabSelectedClass : tabIdleClass)}
            type="button"
            onClick={() => setMode("oauth")}
            role="tab"
            aria-selected={mode === "oauth"}
          >
            <span className={tabContentClass}>OAuth 导入</span>
          </button>
          <button
            className={cx(tabClass, mode === "refreshToken" ? tabSelectedClass : tabIdleClass)}
            type="button"
            onClick={() => setMode("refreshToken")}
            role="tab"
            aria-selected={mode === "refreshToken"}
          >
            <span className={tabContentClass}>RT 导入</span>
          </button>
        </SlidingTabList>
      )}

      {mode === "oauth" ? (
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
            <form className={fieldStack} onSubmit={event => { event.preventDefault(); props.onSubmitCallback(callbackUrl); }}>
              <label className={fieldLabel} htmlFor="callbackUrl">
                {isClaude ? "Authorization Result" : "Callback URL"}
              </label>
              <textarea
                className={textareaClass}
                id="callbackUrl"
                value={callbackUrl}
                onChange={(event) => setCallbackUrl(event.target.value)}
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
                disabled={props.saving || callbackUrl.trim().length === 0}
              >
                {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
                确认
              </button>
            </form>
          </div>
        </div>
      ) : (
        <div>
          <form className="grid gap-4" onSubmit={event => { event.preventDefault(); props.onSubmitManual({ refreshToken, clientId, chatgptAccountId }); }}>
            <label className={fieldStack}>
              <span className={fieldLabel}>Client ID</span>
              <input
                className={inputClass}
                value={clientId}
                onChange={(event) => setClientId(event.target.value)}
                placeholder={defaultGptClientId}
                autoComplete="off"
                maxLength={512}
              />
            </label>
            <label className={fieldStack}>
              <span className={fieldLabel}>ChatGPT Account ID</span>
              <input
                className={inputClass}
                value={chatgptAccountId}
                onChange={(event) => setChatgptAccountId(event.target.value)}
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
                value={refreshToken}
                onChange={(event) => setRefreshToken(event.target.value)}
                rows={5}
                placeholder="粘贴 refresh_token"
                maxLength={32 * 1024}
                required
              />
            </label>
            <button
              className={`${buttonPrimary} mt-1 w-full`}
              disabled={props.saving || refreshToken.trim().length === 0}
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

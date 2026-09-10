import { Eye, EyeOff } from "lucide-react";
import { useId, useState, type ComponentProps } from "react";
import { cx, inputClass } from "../lib/ui";

type PasswordInputProps = Omit<ComponentProps<"input">, "type">;

export function PasswordInput({ id, className, disabled, ...props }: PasswordInputProps) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const [visible, setVisible] = useState(false);
  const toggleLabel = visible ? "隐藏密码" : "显示密码";

  return (
    <span className="relative block min-w-0">
      <input
        {...props}
        id={inputId}
        className={cx(inputClass, className, "pr-11")}
        type={visible ? "text" : "password"}
        disabled={disabled}
      />
      <button
        type="button"
        className="absolute inset-y-0 right-1 my-auto inline-flex size-8 items-center justify-center rounded-md text-slate-500 transition-colors hover:bg-slate-100 hover:text-slate-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-600/30 disabled:pointer-events-none disabled:opacity-50 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-200 dark:focus-visible:ring-indigo-400/35"
        disabled={disabled}
        aria-label={toggleLabel}
        aria-controls={inputId}
        title={toggleLabel}
        onClick={() => setVisible(current => !current)}
      >
        {visible ? <EyeOff size={17} aria-hidden="true" /> : <Eye size={17} aria-hidden="true" />}
      </button>
    </span>
  );
}

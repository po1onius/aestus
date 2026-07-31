import { X } from "lucide-react";
import { m } from "motion/react";
import { useEffect, useRef, type PointerEvent, type ReactNode } from "react";
import { cx, iconButton } from "../lib/ui";

interface ModalProps {
  titleId: string;
  title: ReactNode;
  description?: ReactNode;
  className?: string;
  role?: "dialog" | "alertdialog";
  ariaDescribedBy?: string;
  closeDisabled?: boolean;
  onClose: () => void;
  children: ReactNode;
}

const modalEnterEase = [0.22, 1, 0.36, 1] as const;
const modalExitEase = [0.4, 0, 1, 1] as const;

/** Dashboard 弹窗统一壳层，集中维护 backdrop、键盘交互、可访问性属性和关闭行为。 */
export function Modal({
  titleId,
  title,
  description,
  className,
  role = "dialog",
  ariaDescribedBy,
  closeDisabled,
  onClose,
  children,
}: ModalProps) {
  const descriptionId = description ? `${titleId}Description` : undefined;
  const backdropPointerIdRef = useRef<number | null>(null);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape" || closeDisabled) {
        return;
      }
      event.preventDefault();
      onClose();
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [closeDisabled, onClose]);

  function requestClose() {
    // 提交过程中同时锁定关闭按钮、遮罩层和 Escape，避免请求仍在执行时销毁表单状态。
    if (!closeDisabled) {
      onClose();
    }
  }

  function handleBackdropPointerDown(event: PointerEvent<HTMLDivElement>) {
    // 只有直接按在遮罩空白处才记录关闭意图；从弹窗内部开始的拖动不能关闭弹窗。
    backdropPointerIdRef.current =
      event.target === event.currentTarget ? event.pointerId : null;
  }

  function handleBackdropPointerUp(event: PointerEvent<HTMLDivElement>) {
    const startedOnBackdrop = backdropPointerIdRef.current === event.pointerId;
    backdropPointerIdRef.current = null;
    if (startedOnBackdrop && event.target === event.currentTarget) {
      requestClose();
    }
  }

  function handleBackdropPointerCancel(event: PointerEvent<HTMLDivElement>) {
    if (backdropPointerIdRef.current === event.pointerId) {
      backdropPointerIdRef.current = null;
    }
  }

  return (
    <m.div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-hidden bg-slate-950/40 px-3 py-[6dvh] backdrop-blur-[1px] sm:px-6 sm:py-[8dvh]"
      role="presentation"
      initial={{ opacity: 0 }}
      animate={{
        opacity: 1,
        transition: { duration: 0.16, ease: modalEnterEase },
      }}
      exit={{
        opacity: 0,
        transition: { duration: 0.12, ease: modalExitEase },
      }}
      onPointerDown={handleBackdropPointerDown}
      onPointerUp={handleBackdropPointerUp}
      onPointerCancel={handleBackdropPointerCancel}
    >
      <m.section
        className={cx(
          "flex max-h-[88dvh] w-full origin-top flex-col gap-5 rounded-xl border border-slate-200 bg-white p-5 text-slate-950 shadow-xl shadow-slate-950/10 will-change-transform dark:border-slate-800 dark:bg-slate-900 dark:text-slate-100 dark:shadow-black/40 sm:max-h-[84dvh] sm:p-6",
          className ?? "max-w-2xl",
        )}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={ariaDescribedBy ?? descriptionId}
        initial={{ opacity: 0, y: 8, scale: 0.985 }}
        animate={{
          opacity: 1,
          y: 0,
          scale: 1,
          transition: { duration: 0.18, ease: modalEnterEase },
        }}
        exit={{
          opacity: 0,
          y: 5,
          scale: 0.99,
          transition: { duration: 0.12, ease: modalExitEase },
        }}
      >
        <div className="flex shrink-0 items-start justify-between gap-5">
          <div className="min-w-0">
            <h2 id={titleId} className="text-lg font-semibold tracking-tight text-slate-950 dark:text-slate-100">
              {title}
            </h2>
            {description && (
              <p id={descriptionId} className="mt-1.5 text-sm leading-6 text-slate-500 dark:text-slate-400">
                {description}
              </p>
            )}
          </div>
          <button
            type="button"
            className={iconButton}
            onClick={requestClose}
            disabled={closeDisabled}
            title="关闭"
            aria-label="关闭弹窗"
          >
            <X size={18} />
          </button>
        </div>
        {/* 标题区固定在弹窗顶部，只有业务内容超过可用高度时才在弹窗内部滚动。 */}
        <div className="-m-1 min-h-0 overflow-y-auto overscroll-contain p-1">{children}</div>
      </m.section>
    </m.div>
  );
}

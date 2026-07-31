import anthropicLogo from "@lobehub/icons-static-svg/icons/claude-color.svg";
import openAiLogo from "@lobehub/icons-static-svg/icons/openai.svg";
import { Check, ChevronDown } from "lucide-react";
import { AnimatePresence, m } from "motion/react";
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type FocusEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { createPortal } from "react-dom";
import { cx } from "../lib/ui";
import type { ProviderGroupReference, UpstreamApiKeyProvider } from "../types";

interface ProviderGroupPickerProps {
  groups: ProviderGroupReference[];
  value: string;
  disabled?: boolean;
  ariaLabel?: string;
  title?: string;
  className?: string;
  onChange: (groupId: string) => void;
}

interface DropdownPosition {
  left: number;
  width: number;
  maxHeight: number;
  placement: "above" | "below";
  edge: number;
}

const providerLogos: Record<UpstreamApiKeyProvider, string> = {
  gpt: openAiLogo,
  claude: anthropicLogo,
};

const providerLabels: Record<UpstreamApiKeyProvider, string> = {
  gpt: "OpenAI",
  claude: "Claude",
};

const dropdownEnterEase = [0.22, 1, 0.36, 1] as const;
const dropdownExitEase = [0.4, 0, 1, 1] as const;
const dropdownGap = 6;
const viewportPadding = 8;
const preferredMenuHeight = 240;

/**
 * Dashboard 共用的 Provider 分组选择器。
 *
 * 下拉层通过 Portal 挂到 document.body，避免账号表格的横向滚动容器裁剪菜单；组件仍统一
 * 维护开合动画、焦点转移、方向键导航和点击外部关闭行为，业务页面只需要传入分组数据。
 */
export function ProviderGroupPicker({
  groups,
  value,
  disabled = false,
  ariaLabel,
  title,
  className,
  onChange,
}: ProviderGroupPickerProps) {
  const [open, setOpen] = useState(false);
  const [dropdownPosition, setDropdownPosition] = useState<DropdownPosition | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const requestedFocusIndexRef = useRef<number | null>(null);
  const listboxId = useId();
  const selectedIndex = groups.findIndex((group) => group.id === value);
  const selectedGroup = selectedIndex >= 0 ? groups[selectedIndex] : null;

  const updateDropdownPosition = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) {
      return;
    }

    const rect = trigger.getBoundingClientRect();
    const availableBelow = Math.max(
      0,
      window.innerHeight - rect.bottom - dropdownGap - viewportPadding,
    );
    const availableAbove = Math.max(0, rect.top - dropdownGap - viewportPadding);
    const expectedHeight = Math.min(preferredMenuHeight, groups.length * 44 + 12);
    const placement =
      availableBelow >= expectedHeight || availableBelow >= availableAbove ? "below" : "above";
    const availableHeight = placement === "below" ? availableBelow : availableAbove;
    const width = Math.min(rect.width, window.innerWidth - viewportPadding * 2);
    const left = Math.min(
      Math.max(rect.left, viewportPadding),
      window.innerWidth - viewportPadding - width,
    );

    setDropdownPosition({
      left,
      width,
      maxHeight: Math.min(preferredMenuHeight, availableHeight),
      placement,
      edge:
        placement === "below"
          ? rect.bottom + dropdownGap
          : window.innerHeight - rect.top + dropdownGap,
    });
  }, [groups.length]);

  useLayoutEffect(() => {
    if (open) {
      // 在浏览器绘制菜单前再次读取位置，覆盖点击到渲染之间可能发生的布局变化。
      updateDropdownPosition();
    }
  }, [open, updateDropdownPosition]);

  useEffect(() => {
    if (!open) {
      return;
    }

    const fallbackIndex = Math.max(0, selectedIndex);
    const focusIndex = requestedFocusIndexRef.current ?? fallbackIndex;
    requestedFocusIndexRef.current = null;
    const frame = window.requestAnimationFrame(() => optionRefs.current[focusIndex]?.focus());

    function handlePointerDown(event: PointerEvent) {
      const target = event.target as Node;
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) {
        setOpen(false);
      }
    }

    document.addEventListener("pointerdown", handlePointerDown);
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener("pointerdown", handlePointerDown);
    };
  }, [open, selectedIndex]);

  useEffect(() => {
    if (!open) {
      return;
    }

    let frame: number | null = null;
    function schedulePositionUpdate() {
      if (frame !== null) {
        return;
      }
      frame = window.requestAnimationFrame(() => {
        frame = null;
        updateDropdownPosition();
      });
    }

    // 捕获任意祖先滚动事件，让 Portal 菜单持续贴合表格或页面中的触发按钮。
    window.addEventListener("scroll", schedulePositionUpdate, true);
    window.addEventListener("resize", schedulePositionUpdate);
    return () => {
      if (frame !== null) {
        window.cancelAnimationFrame(frame);
      }
      window.removeEventListener("scroll", schedulePositionUpdate, true);
      window.removeEventListener("resize", schedulePositionUpdate);
    };
  }, [open, updateDropdownPosition]);

  useEffect(() => {
    optionRefs.current.length = groups.length;
    if (disabled || groups.length === 0) {
      setOpen(false);
    }
  }, [disabled, groups.length]);

  function openDropdown(focusIndex = Math.max(0, selectedIndex)) {
    requestedFocusIndexRef.current = focusIndex;
    updateDropdownPosition();
    setOpen(true);
  }

  function selectGroup(group: ProviderGroupReference) {
    onChange(group.id);
    setOpen(false);
    triggerRef.current?.focus();
  }

  function handleTriggerKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>) {
    if (open || groups.length === 0) {
      return;
    }

    let focusIndex: number | null = null;
    if (event.key === "ArrowDown") {
      focusIndex = Math.max(0, selectedIndex);
    } else if (event.key === "ArrowUp") {
      focusIndex = selectedIndex >= 0 ? selectedIndex : groups.length - 1;
    } else if (event.key === "Home") {
      focusIndex = 0;
    } else if (event.key === "End") {
      focusIndex = groups.length - 1;
    }

    if (focusIndex !== null) {
      event.preventDefault();
      openDropdown(focusIndex);
    }
  }

  function handleOptionKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>, index: number) {
    let nextIndex: number | null = null;
    if (event.key === "ArrowDown") {
      nextIndex = (index + 1) % groups.length;
    } else if (event.key === "ArrowUp") {
      nextIndex = (index - 1 + groups.length) % groups.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = groups.length - 1;
    }

    if (nextIndex !== null) {
      event.preventDefault();
      optionRefs.current[nextIndex]?.focus();
    }
  }

  function handleRootKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key !== "Escape" || !open) {
      return;
    }

    // Escape 只关闭当前下拉框，不继续冒泡到 Modal 并把整个业务弹窗一起关闭。
    event.preventDefault();
    event.stopPropagation();
    setOpen(false);
    triggerRef.current?.focus();
  }

  function handleRootBlur(event: FocusEvent<HTMLDivElement>) {
    const nextTarget = event.relatedTarget;
    if (
      !(nextTarget instanceof Node) ||
      (!rootRef.current?.contains(nextTarget) && !menuRef.current?.contains(nextTarget))
    ) {
      setOpen(false);
    }
  }

  const resolvedAriaLabel =
    ariaLabel ??
    (selectedGroup
      ? `Provider 分组：${selectedGroup.name}，${providerLabels[selectedGroup.provider]}`
      : "请选择 Provider 分组");

  return (
    <div
      className={cx("relative", className)}
      ref={rootRef}
      onBlur={handleRootBlur}
      onKeyDown={handleRootKeyDown}
    >
      <button
        ref={triggerRef}
        className="grid min-h-10 w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-lg border border-slate-300 bg-white px-2.5 py-1.5 text-left text-sm text-slate-900 shadow-xs outline-none transition hover:border-slate-400 focus-visible:border-indigo-600 focus-visible:ring-3 focus-visible:ring-indigo-600/12 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100 dark:hover:border-slate-600 dark:focus-visible:border-indigo-400 dark:focus-visible:ring-indigo-400/18"
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        aria-label={resolvedAriaLabel}
        title={title}
        disabled={disabled || groups.length === 0}
        onClick={() => {
          if (open) {
            setOpen(false);
          } else {
            openDropdown();
          }
        }}
        onKeyDown={handleTriggerKeyDown}
      >
        {selectedGroup ? (
          <>
            <ProviderLogo provider={selectedGroup.provider} />
            <span className="truncate">{selectedGroup.name}</span>
          </>
        ) : (
          <span className="col-span-2 text-slate-400 dark:text-slate-500">请选择分组</span>
        )}
        <ChevronDown
          className={cx(
            "text-slate-500 transition-transform duration-200 ease-out dark:text-slate-400",
            open && "rotate-180",
          )}
          size={17}
        />
      </button>

      {typeof document !== "undefined" &&
        createPortal(
          <AnimatePresence>
            {open && dropdownPosition && (
              <m.div
                ref={menuRef}
                id={listboxId}
                className={cx(
                  "fixed z-[70] grid gap-1 overflow-y-auto rounded-lg border border-slate-200 bg-white p-1.5 shadow-xl shadow-slate-950/10 will-change-[clip-path,transform,opacity] dark:border-slate-700 dark:bg-slate-900 dark:shadow-black/40",
                  dropdownPosition.placement === "below" ? "origin-top" : "origin-bottom",
                )}
                style={dropdownStyle(dropdownPosition)}
                role="listbox"
                aria-label="Provider 分组"
                initial={closedAnimation(dropdownPosition.placement, true)}
                animate={{
                  opacity: 1,
                  y: 0,
                  scale: 1,
                  clipPath: "inset(0 0 0% 0 round 0.5rem)",
                  transition: { duration: 0.18, ease: dropdownEnterEase },
                }}
                exit={{
                  ...closedAnimation(dropdownPosition.placement, false),
                  pointerEvents: "none",
                  transition: { duration: 0.13, ease: dropdownExitEase },
                }}
              >
                {groups.map((group, index) => {
                  const selected = group.id === value;
                  return (
                    <button
                      key={group.id}
                      ref={(element) => {
                        optionRefs.current[index] = element;
                      }}
                      className={cx(
                        "grid min-h-10 w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-md border px-2 py-1.5 text-left text-sm outline-none transition-colors duration-200 [&>*]:transition-transform [&>*]:duration-200 hover:[&>*]:-translate-y-px focus-visible:ring-2 focus-visible:ring-indigo-600/25",
                        selected
                          ? "border-indigo-200 bg-indigo-50 font-semibold text-indigo-800 dark:border-indigo-800 dark:bg-indigo-950/70 dark:text-indigo-300"
                          : "border-transparent text-slate-700 dark:text-slate-300",
                      )}
                      type="button"
                      role="option"
                      aria-label={`${group.name}，${providerLabels[group.provider]}`}
                      aria-selected={selected}
                      onClick={() => selectGroup(group)}
                      onKeyDown={(event) => handleOptionKeyDown(event, index)}
                    >
                      <ProviderLogo provider={group.provider} />
                      <span className="truncate">{group.name}</span>
                      {selected && (
                        <Check className="text-indigo-700 dark:text-indigo-400" size={17} />
                      )}
                    </button>
                  );
                })}
              </m.div>
            )}
          </AnimatePresence>,
          document.body,
        )}
    </div>
  );
}

function dropdownStyle(position: DropdownPosition): CSSProperties {
  return {
    left: position.left,
    width: position.width,
    maxHeight: position.maxHeight,
    ...(position.placement === "below"
      ? { top: position.edge }
      : { bottom: position.edge }),
  };
}

function closedAnimation(placement: DropdownPosition["placement"], entering: boolean) {
  const openingBelow = placement === "below";
  return {
    opacity: 0,
    y: openingBelow ? (entering ? -5 : -4) : entering ? 5 : 4,
    scale: entering ? 0.985 : 0.99,
    clipPath: openingBelow
      ? "inset(0 0 100% 0 round 0.5rem)"
      : "inset(100% 0 0 0 round 0.5rem)",
  };
}

function ProviderLogo({ provider }: { provider: UpstreamApiKeyProvider }) {
  return (
    <span
      className={cx(
        "grid size-7 shrink-0 place-items-center rounded-md border",
        provider === "claude"
          ? "border-orange-200 bg-orange-50 dark:border-orange-900 dark:bg-orange-950/50"
          : "border-slate-200 bg-slate-100 dark:border-slate-700 dark:bg-slate-800",
      )}
      aria-hidden="true"
    >
      <img className="size-4.5" src={providerLogos[provider]} alt="" />
    </span>
  );
}

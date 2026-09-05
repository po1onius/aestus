import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { Loader2, Triangle, type LucideIcon } from "lucide-react";
import { Fragment, useState } from "react";
import { cx, spinnerClass } from "../lib/ui";

export interface RowAction {
  id: string;
  label: string;
  icon?: LucideIcon;
  disabled?: boolean;
  hidden?: boolean;
  danger?: boolean;
  description?: string;
  /** 打开弹窗或行内编辑时，由目标界面接管焦点。 */
  opensDialog?: boolean;
  onSelect: () => void;
}

interface RowActionsProps {
  resourceLabel: string;
  busy?: boolean;
  actions: RowAction[];
}

const actionButtonClass =
  "relative inline-flex h-8 shrink-0 cursor-pointer items-center justify-center gap-1.5 whitespace-nowrap border border-slate-300 bg-slate-50 text-xs font-medium text-slate-700 transition-colors hover:bg-slate-100 hover:text-slate-950 focus-visible:z-10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500 disabled:cursor-not-allowed disabled:opacity-50 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-200 dark:hover:bg-slate-700 dark:hover:text-white";

/** 行内仅显示展开按钮，全部操作集中在菜单中；Portal 避免菜单被表格滚动容器裁切。 */
export function RowActions({ resourceLabel, busy, actions }: RowActionsProps) {
  const [pendingAction, setPendingAction] = useState<RowAction | null>(null);
  const visibleActions = actions.filter((action) => !action.hidden);
  const hasMenu = visibleActions.length > 0;

  function selectAction(action: RowAction) {
    // 不记录资源名称、凭证或表单内容，操作结果继续由业务控制器记录。
    console.debug("[dashboard] row action selected", { action: action.id });
    action.onSelect();
  }

  return (
    <div className="flex items-center justify-end whitespace-nowrap" aria-busy={busy || undefined}>
      {hasMenu && (
        <DropdownMenu.Root modal={false}>
          <DropdownMenu.Trigger asChild>
            <button
              type="button"
              className={cx(
                actionButtonClass,
                "group/row-actions w-8 rounded-md px-0 data-[state=open]:bg-slate-200 dark:data-[state=open]:bg-slate-700",
              )}
              disabled={busy || pendingAction !== null}
              aria-label={`${resourceLabel}的更多操作`}
              title="展开更多操作"
            >
              {busy ? (
                <Loader2 size={16} className={spinnerClass} aria-hidden="true" />
              ) : (
                <Triangle size={10} className="rotate-180 transition-transform duration-150 ease-out group-data-[state=open]/row-actions:rotate-0 motion-reduce:transition-none" fill="currentColor" strokeWidth={0} aria-hidden="true" />
              )}
            </button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content
              align="end"
              sideOffset={5}
              collisionPadding={8}
              loop
              className="row-actions-menu z-50 grid w-max max-w-[calc(100vw-1rem)] max-h-[var(--radix-dropdown-menu-content-available-height)] gap-1.5 overflow-y-auto rounded-lg border border-slate-200 bg-white p-2 shadow-lg shadow-slate-950/10 dark:border-slate-700 dark:bg-slate-900 dark:shadow-black/30"
              onCloseAutoFocus={(event) => {
                if (pendingAction) {
                  // 等待收起和卸载完成后再打开目标界面，避免菜单的焦点事件抢回输入焦点。
                  event.preventDefault();
                  setPendingAction(null);
                  selectAction(pendingAction);
                }
              }}
            >
              {visibleActions.map((action, index) => {
                const Icon = action.icon;
                return (
                  <Fragment key={action.id}>
                    {action.danger && index > 0 && !visibleActions[index - 1].danger && (
                      <DropdownMenu.Separator className="h-px bg-slate-100 dark:bg-slate-800" />
                    )}
                    <DropdownMenu.Item
                      disabled={busy || pendingAction !== null || action.disabled}
                      textValue={action.label}
                      title={action.description}
                      className={cx(
                        "flex min-h-8 cursor-pointer select-none items-center justify-center gap-1.5 rounded-md border px-2.5 py-1.5 text-center text-xs font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-indigo-500 data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
                        action.danger
                          ? "border-red-200 bg-red-50 text-red-700 data-[highlighted]:bg-red-100 dark:border-red-900 dark:bg-red-950/40 dark:text-red-300 dark:data-[highlighted]:bg-red-900/50"
                          : "border-slate-300 bg-slate-50 text-slate-700 data-[highlighted]:bg-slate-100 data-[highlighted]:text-slate-950 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-200 dark:data-[highlighted]:bg-slate-700 dark:data-[highlighted]:text-white",
                      )}
                      onSelect={() => {
                        if (action.opensDialog) {
                          setPendingAction(action);
                        } else {
                          selectAction(action);
                        }
                      }}
                    >
                      {Icon && <Icon size={14} className="shrink-0" aria-hidden="true" />}
                      {action.label}
                    </DropdownMenu.Item>
                  </Fragment>
                );
              })}
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      )}
    </div>
  );
}

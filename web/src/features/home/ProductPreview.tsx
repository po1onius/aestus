import { useState } from "react";
import {
  Activity,
  ArrowUpRight,
  Check,
  ChevronRight,
  KeyRound,
  Layers3,
  ShieldCheck,
  Users,
} from "lucide-react";
import openaiLogo from "@lobehub/icons-static-svg/icons/openai.svg";
import tokenGatewayLogo from "../../assets/token-gateway-logo.svg";
import { consolePagePaths } from "../../config";

const scenes = [
  {
    id: "resources",
    label: "账号资源",
    icon: Layers3,
    title: "资源清晰，调度有序。",
    description: "在同一个工作空间，管理账号分组、运行状态与凭证。",
    caption: "统一管理自有账号，按资源组服务不同业务。",
  },
  {
    id: "members",
    label: "成员授权",
    icon: Users,
    title: "各有所用，各有边界。",
    description: "按组授予资源访问权限，为成员设置额度与并发。",
    caption: "成员使用独立的网关 Key，无需共享上游账号凭证。",
  },
  {
    id: "requests",
    label: "请求洞察",
    icon: Activity,
    title: "每次调用，都有迹可循。",
    description: "从请求记录到用量明细，理解团队的 AI 资源消耗。",
    caption: "管理员查看空间内的调用，成员查看自己的记录。",
  },
] as const;

type SceneId = (typeof scenes)[number]["id"];

function ResourcePreview() {
  return (
    <>
      <div className="preview-metrics">
        <div>
          <span>托管账号</span>
          <strong>
            03 <small>个</small>
          </strong>
        </div>
        <div>
          <span>可用资源</span>
          <strong>
            03 <small className="preview-positive">全部就绪</small>
          </strong>
        </div>
        <div>
          <span>资源分组</span>
          <strong>
            02 <small>个</small>
          </strong>
        </div>
      </div>
      <div
        className="preview-table-scroll"
        tabIndex={0}
        role="region"
        aria-label="账号资源演示表，可横向滚动"
      >
        <table className="preview-table">
          <thead>
            <tr>
              <th>GPT 账号</th>
              <th>所在组</th>
              <th>状态</th>
              <th>凭证</th>
            </tr>
          </thead>
          <tbody>
            {[
              ["studio-01@example.com", "研发团队"],
              ["studio-02@example.com", "研发团队"],
              ["creative@example.com", "创意团队"],
            ].map(([email, group]) => (
              <tr key={email}>
                <td>
                  <div className="preview-account">
                    <img src={openaiLogo} alt="" />
                    <div>
                      <strong>{email}</strong>
                      <span>OAuth 账号</span>
                    </div>
                  </div>
                </td>
                <td>
                  <span className="preview-tag">{group}</span>
                </td>
                <td>
                  <span className="preview-positive">
                    <i />
                    就绪
                  </span>
                </td>
                <td>
                  <span className="preview-credential">
                    <Check size={14} />
                    有效
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

function MemberPreview() {
  return (
    <>
      <div className="preview-metrics">
        <div>
          <span>空间成员</span>
          <strong>
            03 <small>人</small>
          </strong>
        </div>
        <div>
          <span>可授权分组</span>
          <strong>
            02 <small>个</small>
          </strong>
        </div>
        <div>
          <span>授权方式</span>
          <strong className="preview-metric-text">按组分配</strong>
        </div>
      </div>
      <div
        className="preview-table-scroll"
        tabIndex={0}
        role="region"
        aria-label="成员授权演示表，可横向滚动"
      >
        <table className="preview-table">
          <thead>
            <tr>
              <th>成员</th>
              <th>资源授权</th>
              <th>角色</th>
              <th>GPT 并发上限</th>
            </tr>
          </thead>
          <tbody>
            {[
              ["L", "Lin", "全部资源", "空间管理员", "10"],
              ["C", "Chen", "研发团队", "普通成员", "5"],
              ["Y", "Yu", "创意团队", "普通成员", "3"],
            ].map(([initial, name, group, role, concurrency]) => (
              <tr key={name}>
                <td>
                  <div className="preview-account">
                    <span className="preview-avatar">{initial}</span>
                    <strong>{name}</strong>
                  </div>
                </td>
                <td>
                  <span className="preview-tag">{group}</span>
                </td>
                <td>{role}</td>
                <td className="preview-number">{concurrency}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

function RequestPreview() {
  return (
    <>
      <div className="preview-metrics">
        <div>
          <span>请求状态</span>
          <strong className="preview-metric-text preview-positive">
            200 <small>成功</small>
          </strong>
        </div>
        <div>
          <span>总用量</span>
          <strong>
            2,480 <small>Tokens</small>
          </strong>
        </div>
        <div>
          <span>请求耗时</span>
          <strong>
            3.2 <small>s</small>
          </strong>
        </div>
      </div>
      <div className="preview-request">
        <div className="preview-request-heading">
          <span>
            <span className="preview-method">POST</span> /v1/responses
          </span>
          <span>研发团队 / Chen</span>
        </div>
        <div className="preview-trace">
          {[
            ["01", "请求接入", "网关 Key 鉴权"],
            ["02", "资源调度", "选择可用账号"],
            ["03", "上游处理", "流式响应"],
            ["04", "用量记录", "写入调用明细"],
          ].map(([number, label, detail]) => (
            <div key={number}>
              <span className="preview-trace-dot">
                <Check size={15} />
              </span>
              <small>{number}</small>
              <strong>{label}</strong>
              <span>{detail}</span>
            </div>
          ))}
        </div>
        <div className="preview-request-id">
          <span>REQUEST ID</span>
          <code>019a…7e2c</code>
          <span>请求明细 · 示例</span>
        </div>
      </div>
    </>
  );
}

/** 首页场景演示仅使用固定展示数据，不加载业务接口或挂载管理页面。 */
export function ProductPreview() {
  const [active, setActive] = useState<SceneId>("resources");
  const scene = scenes.find((item) => item.id === active)!;
  function selectScene(id: SceneId) {
    if (id === active) return;
    console.debug("[home] Product preview scene changed", { scene: id });
    setActive(id);
  }
  return (
    <section
      className="home-product home-container"
      id="product"
      aria-labelledby="product-title"
    >
      <div className="home-section-heading">
        <div>
          <div className="home-eyebrow">
            <span>01</span> 工作空间，一目了然
          </div>
          <h2 id="product-title">
            复杂留给系统。
            <br />
            <span>清晰留给你。</span>
          </h2>
        </div>
        <p>
          从资源到成员，从调用到用量。
          <br />
          在一个井然有序的空间里，掌握全局。
        </p>
      </div>
      <div
        className="preview-scenes"
        role="group"
        aria-label="选择产品演示场景"
      >
        {scenes.map(({ id, label, icon: Icon }) => (
          <button
            type="button"
            key={id}
            aria-pressed={id === active}
            aria-controls="product-preview-panel"
            onClick={() => selectScene(id)}
          >
            <Icon size={17} />
            {label}
            <ChevronRight size={14} />
          </button>
        ))}
      </div>
      <div
        className="product-window"
        id="product-preview-panel"
        role="region"
        aria-label={`${scene.label}演示`}
      >
        <div className="preview-window-bar">
          <div className="preview-window-dots" aria-hidden="true">
            <i />
            <i />
            <i />
          </div>
          <span>aestus / workspace</span>
          <span className="preview-demo-label">交互预览 · 演示数据</span>
        </div>
        <div className="preview-workspace">
          <aside className="preview-sidebar" aria-label="演示空间信息">
            <div className="preview-brand">
              <img src={tokenGatewayLogo} alt="" />
              aestus<span>.</span>
            </div>
            <div className="preview-workspace-name">
              <span className="preview-workspace-avatar">S</span>
              <div>
                <strong>Studio</strong>
                <small>团队工作空间</small>
              </div>
            </div>
            <div className="preview-sidebar-label">工作空间</div>
            {scenes.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                type="button"
                aria-pressed={active === id}
                aria-controls="product-preview-panel"
                onClick={() => selectScene(id)}
              >
                <Icon size={16} />
                {label}
              </button>
            ))}
            <div className="preview-sidebar-bottom">
              <ShieldCheck size={15} />
              资源按空间隔离
            </div>
          </aside>
          <div className="preview-content">
            <div className="preview-breadcrumb">
              Studio <ChevronRight size={12} />
              {scene.label}
              <span>
                <i />
                演示空间
              </span>
            </div>
            <div className="preview-content-heading">
              <div>
                <h3>{scene.label}</h3>
                <p>{scene.description}</p>
              </div>
              <span className="preview-owner">管理员视角</span>
            </div>
            <div className="preview-scene-content" key={active}>
              {active === "resources" ? (
                <ResourcePreview />
              ) : active === "members" ? (
                <MemberPreview />
              ) : (
                <RequestPreview />
              )}
              {active !== "requests" && (
                <p className="preview-scroll-hint">左右滑动，查看完整表格</p>
              )}
            </div>
            <div className="preview-content-footer">
              <ShieldCheck size={14} />
              <span>{scene.caption}</span>
            </div>
          </div>
        </div>
      </div>
      <div className="preview-caption">
        <p>
          <KeyRound size={15} />
          <span aria-live="polite">{scene.title}</span>
        </p>
        <a href={consolePagePaths.usage}>
          进入你的工作空间 <ArrowUpRight size={16} />
        </a>
      </div>
    </section>
  );
}

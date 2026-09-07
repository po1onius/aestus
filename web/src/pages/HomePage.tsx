import { consolePagePaths } from "../config";
import { useState } from "react";
import {
  ArrowDown,
  ArrowDownLeft,
  ArrowRight,
  ArrowUpRight,
  AudioLines,
  Check,
  CheckCheck,
  ChevronRight,
  Code2,
  Copy,
  Fingerprint,
  Image,
  Layers3,
  Menu,
  Network,
  Search,
  ShieldCheck,
  Terminal,
  Workflow,
  X,
} from "lucide-react";
import openaiLogo from "@lobehub/icons-static-svg/icons/openai.svg";
import claudeLogo from "@lobehub/icons-static-svg/icons/claude.svg";
import tokenGatewayLogo from "../assets/token-gateway-logo.svg";
import "./home.css";

const consoleEntryPath = consolePagePaths.usage;
const examples = [
  {
    id: "responses",
    label: "文本生成",
    path: "/v1/responses",
    body: '{\n    "model": "<YOUR_GPT_MODEL>",\n    "input": "用一句话介绍 Aestus",\n    "stream": true\n  }',
  },
  {
    id: "messages",
    label: "Claude",
    path: "/v1/messages",
    body: '{\n    "model": "<YOUR_CLAUDE_MODEL>",\n    "max_tokens": 1024,\n    "messages": [{"role": "user", "content": "你好，Aestus"}]\n  }',
  },
  {
    id: "images",
    label: "图像生成",
    path: "/v1/images/generations",
    body: '{\n    "model": "gpt-image-2",\n    "prompt": "暖白背景上的极简建筑摄影",\n    "size": "1024x1024"\n  }',
  },
] as const;

function Brand() {
  return (
    <a className="home-brand" href="/" aria-label="Aestus 首页">
      <img src={tokenGatewayLogo} alt="" />
      <span>
        aestus<span className="brand-period">.</span>
      </span>
    </a>
  );
}

function GatewayDiagram() {
  return (
    <div
      className="gateway-diagram"
      role="img"
      aria-label="工作空间账号托管示意：团队成员与应用使用各自的网关 Key，经 Aestus 分组鉴权，调用本工作空间托管的 GPT 与 Claude 账号池。账号资源在工作空间之间隔离。"
    >
      <div className="diagram-heading">
        <span>
          <i /> WORKSPACE ACCOUNT HOSTING
        </span>
        <span>
          架构示意 <ArrowUpRight size={13} />
        </span>
      </div>
      <div className="diagram-grid" />
      <div className="diagram-tenant-boundary">
        <span>
          <ShieldCheck size={12} /> 专属工作空间 · 账号托管
        </span>
      </div>
      <svg
        className="diagram-lines"
        viewBox="0 0 580 400"
        fill="none"
        aria-hidden="true"
      >
        <path d="M100 118H153Q177 118 177 142V188Q177 210 205 210H280M100 210H280M100 302H153Q177 302 177 278V234Q177 210 205 210" />
        <path d="M303 210H356Q386 210 386 180V153Q386 130 412 130H488M303 210H356Q386 210 386 240V267Q386 290 412 290H488" />
        <path
          className="flow-path"
          d="M100 210H280M303 210H356Q386 210 386 180V153Q386 130 412 130H488"
        />
        <circle cx="177" cy="210" r="4" />
        <circle cx="386" cy="210" r="4" />
      </svg>
      <div className="diagram-input input-one">
        <Code2 size={17} />
        <span>团队成员</span>
      </div>
      <div className="diagram-input input-two">
        <Terminal size={17} />
        <span>业务应用</span>
      </div>
      <div className="diagram-input input-three">
        <Workflow size={17} />
        <span>团队 Agent</span>
      </div>
      <div className="gateway-orbit orbit-outer" />
      <div className="gateway-orbit orbit-inner" />
      <div className="gateway-core">
        <Network size={38} strokeWidth={1.4} />
        <span>AESTUS</span>
      </div>
      <div className="gateway-caption">网关 Key · 按组授权</div>
      <div className="diagram-provider provider-gpt">
        <img src={openaiLogo} alt="" />
        <div>
          <strong>GPT</strong>
          <span>托管账号池</span>
        </div>
        <i />
      </div>
      <div className="diagram-provider provider-claude">
        <img src={claudeLogo} alt="" />
        <div>
          <strong>Claude</strong>
          <span>托管账号池</span>
        </div>
        <i />
      </div>
      <div className="diagram-event">
        <span>
          <CheckCheck size={14} /> 空间隔离
        </span>
        <span>
          <CheckCheck size={14} /> 凭证维护
        </span>
        <span>
          <CheckCheck size={14} /> 分组授权
        </span>
      </div>
      <div className="diagram-corner corner-tl" />
      <div className="diagram-corner corner-br" />
    </div>
  );
}

export function HomePage() {
  const [menuOpen, setMenuOpen] = useState(false);
  const [exampleIndex, setExampleIndex] = useState(0);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "error">(
    "idle",
  );
  const example = examples[exampleIndex];
  const baseUrl = "<AESTUS_BASE_URL>";
  const code = `curl "${baseUrl}${example.path}" \\\n  -H "Authorization: Bearer <AESTUS_GATEWAY_KEY>" \\\n  -H "Content-Type: application/json" \\\n${example.id === "messages" ? '  -H "anthropic-version: 2023-06-01" \\\n' : ""}  -d '${example.body}'`;

  async function copyExample() {
    try {
      await navigator.clipboard.writeText(code);
      setCopyStatus("copied");
      console.info("[homepage] 接入示例已复制", { endpoint: example.path });
    } catch (error) {
      setCopyStatus("error");
      console.error("[homepage] 接入示例复制失败", {
        endpoint: example.path,
        error,
      });
    }
  }

  return (
    <div className="aestus-home">
      <a className="home-skip-link" href="#main-content">
        跳到主要内容
      </a>
      <header className="home-header">
        <div className="home-container home-header-inner">
          <Brand />
          <nav className="home-desktop-nav" aria-label="主导航">
            <a href="#capabilities">账号托管</a>
            <a href="#architecture">托管流程</a>
            <a href="#integration">
              快速接入 <ArrowUpRight size={12} />
            </a>
          </nav>
          <a className="home-console-link" href={consoleEntryPath}>
            进入控制台 <ArrowUpRight size={16} />
          </a>
          <button
            className="home-menu-toggle"
            type="button"
            aria-expanded={menuOpen}
            aria-controls="home-mobile-nav"
            aria-label={menuOpen ? "关闭导航" : "打开导航"}
            onClick={() => setMenuOpen(!menuOpen)}
          >
            {menuOpen ? <X size={22} /> : <Menu size={22} />}
          </button>
        </div>
        {menuOpen && (
          <nav
            id="home-mobile-nav"
            className="home-mobile-nav"
            aria-label="移动端导航"
            onClick={() => setMenuOpen(false)}
          >
            <a href="#capabilities">账号托管</a>
            <a href="#architecture">托管流程</a>
            <a href="#integration">快速接入</a>
          </nav>
        )}
      </header>

      <main id="main-content">
        <section className="home-hero home-container">
          <div className="hero-copy">
            <div className="home-eyebrow">
              <span className="eyebrow-line" /> YOUR ACCOUNTS. YOUR WORKSPACE.
            </div>
            <h1>
              专属工作空间，
              <br />
              <span>统一托管 AI 账号。</span>
            </h1>
            <p className="hero-description">
              将 GPT 与 Claude 账号托管到专属工作空间。
              <br />
              集中管理团队账号，灵活分配成员权限，
              <br className="desktop-break" />
              让每一次 AI 调用都有序可控。
            </p>
            <div className="hero-actions">
              <a className="home-button home-button-dark" href={consoleEntryPath}>
                开始账号托管 <ArrowUpRight size={18} />
              </a>
              <a className="home-text-link" href="#architecture">
                了解托管方式 <ArrowRight size={17} />
              </a>
            </div>
            <div className="hero-notes">
              <span>
                <Check size={13} /> 自有账号托管
              </span>
              <span>
                <Check size={13} /> 空间资源隔离
              </span>
              <span>
                <Check size={13} /> 成员按组授权
              </span>
            </div>
          </div>
          <GatewayDiagram />
          <div className="hero-bottom">
            <span>YOUR ACCOUNTS. SHARED WITH YOUR TEAM.</span>
            <a href="#capabilities">
              探索 Aestus <ArrowDown size={14} />
            </a>
          </div>
        </section>

        <section className="home-protocols" aria-label="支持的协议和功能">
          <div className="home-container protocols-inner">
            <span className="protocol-label">自有账号托管，标准协议调用</span>
            <div>
              <img src={openaiLogo} alt="" />
              <span>
                OpenAI <small>compatible</small>
              </span>
            </div>
            <div>
              <img src={claudeLogo} alt="" />
              <span>
                Anthropic <small>compatible</small>
              </span>
            </div>
            <div>
              <Image size={22} />
              <span>Image API</span>
            </div>
            <div>
              <Search size={22} />
              <span>Web Search</span>
            </div>
          </div>
        </section>

        <section id="capabilities" className="home-container home-section">
          <div className="section-heading">
            <div>
              <div className="home-eyebrow">01 / BUILT FOR YOUR WORKSPACE</div>
              <h2>
                账号集中托管，<span>团队有序使用。</span>
              </h2>
            </div>
            <p>
              从账号归属，到成员授权与用量管理。
              <br />
              让每个团队拥有专属的 AI 工作空间。
            </p>
          </div>
          <div className="capability-grid">
            <article className="capability-card resource-card">
              <div className="card-top">
                <Network size={22} />
                <span>WORKSPACE ACCOUNT HOSTING</span>
                <ArrowUpRight size={18} />
              </div>
              <h3>自有账号，在专属空间统一托管</h3>
              <p>
                空间管理员导入 GPT、Claude 账号与官方 API Key。
                <br />
                账号按工作空间隔离，统一完成分组、启停与维护。
              </p>
              <div
                className="resource-visual"
                aria-label="本工作空间托管资源示意，包含 GPT 账号、Claude 账号和官方 API Key"
              >
                <div className="resource-group">
                  <span>
                    <Layers3 size={14} /> 本工作空间 · 托管资源
                  </span>
                  <span className="visual-label">示意</span>
                </div>
                {["GPT OAuth 账号", "Claude OAuth 账号", "官方 API Key"].map(
                  (name, index) => (
                    <div className="resource-row" key={index}>
                      <span className="resource-number">0{index + 1}</span>
                      <span>{name}</span>
                      <span className="resource-binding">本空间</span>
                      <span
                        className={
                          index === 0 ? "resource-selected" : "resource-ready"
                        }
                      >
                        {index < 2 ? "账号托管" : "密钥托管"}
                      </span>
                    </div>
                  ),
                )}
              </div>
            </article>
            <article className="capability-card tenant-card">
              <div className="card-top">
                <ShieldCheck size={22} />
                <span>MEMBER ACCESS</span>
                <ArrowUpRight size={18} />
              </div>
              <h3>资源按组授权，成员按需使用</h3>
              <p>
                成员通过自己的网关 Key 调用，无需持有上游凭证。
                <br />
                空间管理员按资源组授权，并设置成员额度与并发。
              </p>
              <div className="tenant-visual">
                <div className="tenant-root">
                  <Fingerprint size={18} />
                  <span>本工作空间 · 资源分组</span>
                  <span className="tenant-owner">管理员</span>
                </div>
                <div className="tenant-branches">
                  <span>
                    <span className="team-avatar">D</span>开发成员
                    <ShieldCheck size={14} />
                  </span>
                  <span>
                    <span className="team-avatar">P</span>产品成员
                    <ShieldCheck size={14} />
                  </span>
                </div>
              </div>
            </article>
            <article className="capability-card compact-card">
              <div className="small-feature-icon">
                <AudioLines size={22} />
              </div>
              <h3>工作空间的每一份用量，都有据可查</h3>
              <p>
                空间管理员查看空间内的请求日志与用量，成员查看自己的调用记录。
                按用户、模型和 API Key 理解资源消耗。
              </p>
              <div className="mini-timeline">
                <span>
                  <i />
                  请求接入
                </span>
                <ChevronRight size={13} />
                <span>
                  <i />
                  上游处理
                </span>
                <ChevronRight size={13} />
                <span>
                  <i />
                  用量记录
                </span>
              </div>
            </article>
            <article className="capability-card compact-card maintenance-card">
              <div className="small-feature-icon">
                <Layers3 size={22} />
              </div>
              <h3>托管之后，维护与调度自动进行</h3>
              <p>
                自动刷新账号凭证、调度可用资源，失败时切换资源重试。
                粘性会话提升缓存命中，让账号池持续服务团队。
              </p>
              <div className="maintenance-flow">
                <span>凭证刷新</span>
                <ArrowRight size={13} />
                <span>资源调度</span>
                <ArrowRight size={13} />
                <span>会话保持</span>
              </div>
            </article>
          </div>
        </section>

        <section
          id="architecture"
          className="home-container architecture-section"
        >
          <div className="architecture-title">
            <div className="home-eyebrow">02 / FROM ACCOUNTS TO TEAM</div>
            <h2>
              从账号托管，
              <br />
              <span>到整个团队的 AI 能力。</span>
            </h2>
            <p>
              空间管理员统一管理账号，成员按授权使用。
              <br />
              三步，让自有资源服务团队。
            </p>
            <a href="#integration" className="home-text-link">
              查看成员接入示例 <ArrowRight size={17} />
            </a>
          </div>
          <ol className="architecture-steps">
            {[
              {
                number: "01",
                title: "托管自有账号",
                text: "空间管理员导入账号或官方 API Key，归入工作空间的资源组。",
                icon: Code2,
              },
              {
                number: "02",
                title: "授权团队成员",
                text: "按 Provider 分组分配使用权限，设置成员额度与并发。",
                icon: Network,
              },
              {
                number: "03",
                title: "通过网关 Key 调用",
                text: "成员在已授权组中创建自己的 Key，以标准 API 使用托管资源。",
                icon: ArrowDownLeft,
              },
            ].map(({ number, title, text, icon: Icon }) => (
              <li key={number}>
                <span className="step-number">{number}</span>
                <div>
                  <h3>{title}</h3>
                  <p>{text}</p>
                </div>
                <Icon size={22} />
              </li>
            ))}
          </ol>
        </section>

        <section id="integration" className="integration-section">
          <div className="home-container integration-inner">
            <div className="integration-copy">
              <div className="home-eyebrow">03 / ONE KEY FOR YOUR WORK</div>
              <h2>
                账号在专属空间托管。
                <br />
                <span>成员用 Key 即可接入。</span>
              </h2>
              <p>
                在已获授权的资源组中创建网关 Key，配置到应用。
                <br />
                无需分发上游账号凭证，沿用熟悉的 API 调用方式。
              </p>
              <a
                className="home-button home-button-light"
                href={consolePagePaths.gatewayApiKeys}
              >
                管理 API Key <ArrowUpRight size={17} />
              </a>
              <span className="integration-note">
                使用平台提供的注册码加入工作空间；首位注册者成为空间管理员。
              </span>
            </div>
            <div className="code-window">
              <div className="code-window-bar">
                <span>
                  <Terminal size={14} /> 第一个请求
                </span>
                <span>cURL</span>
              </div>
              <div className="code-toolbar">
                <div
                  className="code-tabs"
                  role="tablist"
                  aria-label="API 接入示例"
                >
                  {examples.map((item, index) => (
                    <button
                      id={`example-tab-${item.id}`}
                      key={item.id}
                      role="tab"
                      type="button"
                      aria-selected={exampleIndex === index}
                      aria-controls="example-panel"
                      tabIndex={exampleIndex === index ? 0 : -1}
                      className={exampleIndex === index ? "active" : ""}
                      onClick={() => {
                        setExampleIndex(index);
                        setCopyStatus("idle");
                      }}
                      onKeyDown={(event) => {
                        const next =
                          event.key === "ArrowRight"
                            ? (index + 1) % examples.length
                            : event.key === "ArrowLeft"
                              ? (index + examples.length - 1) % examples.length
                              : event.key === "Home"
                                ? 0
                                : event.key === "End"
                                  ? examples.length - 1
                                  : null;
                        if (next !== null) {
                          event.preventDefault();
                          setExampleIndex(next);
                          setCopyStatus("idle");
                          document
                            .getElementById(`example-tab-${examples[next].id}`)
                            ?.focus();
                        }
                      }}
                    >
                      {item.label}
                    </button>
                  ))}
                </div>
                <button
                  className="copy-code"
                  type="button"
                  onClick={() => void copyExample()}
                  aria-label="复制接入示例"
                >
                  {copyStatus === "copied" ? (
                    <Check size={15} />
                  ) : (
                    <Copy size={15} />
                  )}
                  <span>{copyStatus === "copied" ? "已复制" : "复制"}</span>
                </button>
              </div>
              <div
                id="example-panel"
                role="tabpanel"
                aria-labelledby={`example-tab-${example.id}`}
                tabIndex={0}
              >
                <pre>
                  <code>
                    {code.split("\n").map((line, index) => (
                      <span className="code-line" key={index}>
                        <span className="line-number" aria-hidden="true">
                          {index + 1}
                        </span>
                        <span
                          className={
                            line.trim().startsWith('"') ? "code-json" : ""
                          }
                        >
                          {line}
                        </span>
                        {"\n"}
                      </span>
                    ))}
                  </code>
                </pre>
              </div>
              <div className="code-footnote" role="status">
                {copyStatus === "error"
                  ? "复制失败，请直接选中上方代码复制。"
                  : copyStatus === "copied"
                    ? "示例已复制，请替换网关地址、Key 与模型后调用。"
                    : "替换网关地址、Key，并选择已获授权的模型。"}
              </div>
            </div>
          </div>
        </section>

        <section className="home-container final-cta">
          <div>
            <div className="home-eyebrow">YOUR ACCOUNTS. YOUR TEAM.</div>
            <h2>
              把账号托管好。<span>让整个团队，专注用好 AI。</span>
            </h2>
          </div>
          <a className="home-button home-button-dark" href={consoleEntryPath}>
            进入控制台 <ArrowUpRight size={18} />
          </a>
        </section>
      </main>
      <footer className="home-footer home-container">
        <Brand />
        <p>专属空间托管账号，团队共享 AI 能力。</p>
        <span>© {new Date().getFullYear()} Aestus</span>
        <a href="#main-content">回到顶部 ↑</a>
      </footer>
    </div>
  );
}

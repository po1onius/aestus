import { consolePagePaths } from "../config";
import {
  ArrowDown,
  ArrowRight,
  ArrowUpRight,
  AudioLines,
  Blocks,
  Check,
  CheckCheck,
  ChevronRight,
  Code2,
  Fingerprint,
  Image,
  Layers3,
  Network,
  Search,
  ShieldCheck,
  ShieldAlert,
  Terminal,
  TrendingUp,
  WifiOff,
  Workflow,
} from "lucide-react";
import openaiLogo from "@lobehub/icons-static-svg/icons/openai.svg";
import claudeLogo from "@lobehub/icons-static-svg/icons/claude.svg";
import tokenGatewayLogo from "../assets/token-gateway-logo.svg";
import "./home.css";

const consoleEntryPath = consolePagePaths.usage;

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
  return (
    <div className="aestus-home">
      <a className="home-skip-link" href="#main-content">
        跳到主要内容
      </a>
      <header className="home-header">
        <div className="home-container home-header-inner">
          <Brand />
          <nav className="home-header-nav" aria-label="主导航">
            <span>文档</span>
            <span>咨询</span>
          </nav>
          <a className="home-console-link" href={consoleEntryPath}>
            进入控制台 <ArrowUpRight size={16} />
          </a>
        </div>
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
          className="home-container pain-points-section"
          aria-labelledby="pain-points-title"
        >
          <div className="pain-points-intro">
            <div className="home-eyebrow">02 / LESS FRICTION. MORE VALUE.</div>
            <h2 id="pain-points-title">
              解决 <em>3</em> 大痛点，
              <br />
              <span>让 AI 回归生产力。</span>
            </h2>
            <p>
              从套餐成本、共享隐私，到连接质量。
              <br />
              少一些使用负担，多一份专注创造的自由。
            </p>
            <div className="pain-points-topics" aria-label="成本、隐私、连接">
              <span>成本</span>
              <i />
              <span>隐私</span>
              <i />
              <span>连接</span>
            </div>
            <a href="#integration" className="home-text-link">
              探索项目特点 <ArrowRight size={17} />
            </a>
          </div>
          <ol className="pain-points-list">
            {[
              {
                number: "01",
                label: "COST EFFICIENCY",
                title: "低阶套餐，不够划算。",
                text: "高阶套餐的价格线性增长，额度却成倍提升，限制更少、权益更多。低阶套餐看似门槛低，实际性价比未必高。",
                icon: TrendingUp,
              },
              {
                number: "02",
                label: "PRIVACY & SECURITY",
                title: "共享账号，隐私也被共享。",
                text: "直接共享账号，号主的聊天记录也可能对他人可见。个人对话与敏感数据暴露在同一账号下，隐私与数据安全难以保障。",
                icon: ShieldAlert,
              },
              {
                number: "03",
                label: "CONNECTION STABILITY",
                title: "连接不稳，思路随时中断。",
                text: "AI 服务通常以流式响应持续输出内容。低质量的代理网络容易波动、断连，让尚未完成的回答和连贯的工作节奏一起中断。",
                icon: WifiOff,
              },
            ].map(({ number, label, title, text, icon: Icon }) => (
              <li key={number}>
                <span className="pain-point-number">{number}</span>
                <div>
                  <span className="pain-point-label">{label}</span>
                  <h3>{title}</h3>
                  <p>{text}</p>
                </div>
                <span className="pain-point-icon" aria-hidden="true">
                  <Icon size={21} strokeWidth={1.5} />
                </span>
              </li>
            ))}
          </ol>
        </section>

        <section
          id="integration"
          className="integration-section"
          aria-labelledby="integration-title"
        >
          <div className="home-container">
            <div className="integration-heading">
              <div>
                <div className="home-eyebrow">03 / OPEN BY DESIGN</div>
                <h2 id="integration-title">
                  接入熟悉的协议，<br />
                  <span>扩展自己的可能。</span>
                </h2>
              </div>
              <p>
                以标准协议连接应用，以 WASM 插件定制处理逻辑。<br />
                从即刻接入，到按需扩展。
              </p>
            </div>

            <div className="integration-features">
              <article className="integration-card protocol-feature">
                <div className="integration-card-label">
                  <Code2 size={18} strokeWidth={1.5} />
                  <span>STANDARD PROTOCOLS</span>
                </div>
                <h3>熟悉的 API，<br />连接更丰富的 AI 能力。</h3>
                <p className="integration-description">
                  提供 OpenAI 与 Anthropic 风格接口，沿用熟悉的调用方式。
                  配置网关地址与 Key，将 AI 能力接入你的应用与工作流。
                </p>
                <div className="protocol-interface">
                  <div className="protocol-interface-heading">
                    <span>API INTERFACE</span>
                    <span>兼容协议</span>
                  </div>
                  <div className="protocol-endpoint">
                    <img src={openaiLogo} alt="" />
                    <span>OpenAI</span>
                    <code>/v1/responses</code>
                  </div>
                  <div className="protocol-endpoint">
                    <img src={claudeLogo} alt="" />
                    <span>Anthropic</span>
                    <code>/v1/messages</code>
                  </div>
                  <div className="protocol-capabilities">
                    <span><Terminal size={13} />文本生成</span>
                    <span><Image size={13} />图片生成与编辑</span>
                    <span><Search size={13} />Codex 搜索</span>
                  </div>
                </div>
                <a className="integration-card-link" href={consolePagePaths.gatewayApiKeys}>
                  配置 API Key <ArrowUpRight size={16} />
                </a>
              </article>

              <article className="integration-card wasm-feature">
                <div className="integration-card-label">
                  <Blocks size={18} strokeWidth={1.5} />
                  <span>WEBASSEMBLY PLUGINS</span>
                </div>
                <h3>处理逻辑，<br />由你的插件定义。</h3>
                <p className="integration-description">
                  为请求、非流式响应与流式响应编写自定义转换。
                  将插件组合成套件，按 Key 绑定，让协议适配与业务定制灵活落地。
                </p>
                <div
                  className="wasm-visual"
                  role="img"
                  aria-label="插件套件包含请求转换插槽，以及按交付模式选择的非流式或流式响应转换插槽。"
                >
                  <div className="wasm-module">
                    <Blocks size={29} strokeWidth={1.25} />
                    <div><strong>WASM</strong><span>你的插件套件</span></div>
                    <span className="wasm-file">.wasm</span>
                  </div>
                  <div className="wasm-slots">
                    <div><Code2 size={17} /><span>请求转换</span><small>REQUEST</small></div>
                    <div><Layers3 size={17} /><span>非流式响应</span><small>RESPONSE</small></div>
                    <div><AudioLines size={17} /><span>流式响应</span><small>STREAM</small></div>
                  </div>
                </div>
                <div className="wasm-feature-footer">
                  <span>GPT / Claude · OAuth 文本调用</span>
                  <a className="integration-card-link" href={consolePagePaths.plugins}>
                    管理插件 <ArrowUpRight size={16} />
                  </a>
                </div>
              </article>
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

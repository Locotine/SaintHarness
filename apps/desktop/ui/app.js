/* SaintHarness Native Desktop Frontend (Tauri v2 IPC + live EventStore stream) */

(() => {
  "use strict";

  const SLASH_COMMANDS = [
    { cmd: "/plan", desc: "Switch to read-only Plan mode (Landlock RO)" },
    { cmd: "/auto", desc: "Enable Auto mode (sandboxed workspace auto-approve)" },
    { cmd: "/bypass", desc: "Enable Bypass permissions / Full access (no prompts, no sandbox)" },
    { cmd: "/supervised", desc: "Return to Ask for approval mode" },
    { cmd: "/diff", desc: "Toggle Workspace Diff drawer (Ctrl+D)" },
    { cmd: "/btw", desc: "Open ephemeral read-only sidechat (Ctrl+Alt+S)" },
    { cmd: "/tree", desc: "Open Session DAG branch switcher (Ctrl+T)" },
    { cmd: "/board", desc: "Toggle Agent Board overview (Ctrl+G)" },
    { cmd: "/model", desc: "Open Cross-model handoff picker (Alt+M)" },
    { cmd: "/compact", desc: "Run Tier 1 zero-LLM shake compaction" },
    { cmd: "/new", desc: "Create a new session in this workspace (Ctrl+N)" },
    { cmd: "/settings", desc: "Configure API keys and provider endpoints (Ctrl+,)" },
    { cmd: "/theme", desc: "Toggle Dark / Light color theme (Alt+T)" },
  ];

  const ACCESS_MODES = [
    {
      value: "approval-required",
      label: "Ask for approval",
      meta: "Landlock WorkspaceWrite",
      warn: false,
    },
    {
      value: "auto",
      label: "Auto mode",
      meta: "sandboxed auto-approve",
      warn: false,
    },
    {
      value: "full-access",
      label: "Bypass permissions",
      meta: "full-access · no sandbox",
      warn: true,
    },
  ];

  const STATUS_GROUPS = [
    { key: "blocked", label: "Needs you" },
    { key: "working", label: "Working" },
    { key: "done", label: "Ready" },
    { key: "idle", label: "Idle" },
  ];

  const state = {
    version: "0.1.0",
    workspace: { path: "", name: "Workspace" },
    defaultProfile: "qwen-3.8-27b",
    daemonOk: false,
    daemonError: null,
    sessions: [],
    activeSessionId: null,
    models: { leaves: [], providers: [] },
    expandedProviders: new Set(["claude", "gemini"]),
    auth: {
      settings_path: "~/.saint-harness/settings.json",
      default_profile: "qwen-3.8-27b",
      has_any_configured_key: false,
      providers: [],
    },
    selectedProviderId: "anthropic",
    sidebarOpen: window.innerWidth > 820,
    boardOpen: false,
    diffOpen: false,
    diffData: null,
    diffLoading: false,
    activeOverlay: null, // "model" | "access" | "cache" | "btw" | "tree" | "settings" | null
    overlayTriggerEl: null,
    slashMatches: [],
    slashIndex: 0,
    steerBanner: null,
    theme: "dark",
    toastTimer: null,
  };

  // Tauri bridge helpers
  function hasTauri() {
    return Boolean(window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke);
  }

  async function invoke(cmd, args = {}) {
    if (!hasTauri()) {
      throw new Error("Tauri desktop runtime is not attached in this browser preview.");
    }
    return window.__TAURI__.core.invoke(cmd, args);
  }

  async function pickFolderNative(defaultPath) {
    if (window.__TAURI__ && window.__TAURI__.dialog && window.__TAURI__.dialog.open) {
      return window.__TAURI__.dialog.open({
        directory: true,
        multiple: false,
        defaultPath: defaultPath || undefined,
        title: "Open Workspace Folder",
      });
    }
    return null;
  }

  // DOM helpers
  const $ = (id) => document.getElementById(id);

  function showToast(msg) {
    const el = $("toast");
    el.textContent = msg;
    el.classList.remove("hidden");
    if (state.toastTimer) clearTimeout(state.toastTimer);
    state.toastTimer = setTimeout(() => {
      el.classList.add("hidden");
    }, 2800);
  }

  function activeSession() {
    return state.sessions.find((s) => s.id === state.activeSessionId) || null;
  }

  function applyTheme(mode) {
    state.theme = mode === "light" ? "light" : "dark";
    document.documentElement.classList.toggle("dark", state.theme === "dark");
    $("hdr-theme-label").textContent = state.theme === "dark" ? "Light" : "Dark";
    try {
      localStorage.setItem("saint-harness-desktop-theme", state.theme);
    } catch (_) {}
  }

  function formatRuntimeLabel(mode) {
    if (mode === "auto") return "Auto mode";
    if (mode === "full-access") return "Bypass permissions";
    return "Ask for approval";
  }

  function closeOverlays() {
    const prev = state.activeOverlay;
    state.activeOverlay = null;
    ["model-popover", "access-popover", "cache-popover", "btw-sheet", "tree-modal", "settings-modal"].forEach((id) => {
      $(id).classList.add("hidden");
    });
    $("btn-model").setAttribute("aria-expanded", "false");
    $("btn-access").setAttribute("aria-expanded", "false");
    $("btn-cache").setAttribute("aria-expanded", "false");
    $("btn-settings").setAttribute("aria-expanded", "false");
    if (prev && state.overlayTriggerEl && typeof state.overlayTriggerEl.focus === "function") {
      state.overlayTriggerEl.focus();
    }
    state.overlayTriggerEl = null;
  }

  function openOverlay(name, triggerEl) {
    if (state.activeOverlay === name) {
      closeOverlays();
      return;
    }
    closeOverlays();
    state.activeOverlay = name;
    state.overlayTriggerEl = triggerEl || document.activeElement;

    if (name === "model") {
      renderModelPopover();
      $("model-popover").classList.remove("hidden");
      $("btn-model").setAttribute("aria-expanded", "true");
    } else if (name === "access") {
      renderAccessPopover();
      $("access-popover").classList.remove("hidden");
      $("btn-access").setAttribute("aria-expanded", "true");
    } else if (name === "cache") {
      renderCachePopover();
      $("cache-popover").classList.remove("hidden");
      $("btn-cache").setAttribute("aria-expanded", "true");
    } else if (name === "btw") {
      $("btw-sheet").classList.remove("hidden");
      $("btw-input").focus();
    } else if (name === "tree") {
      $("tree-modal").classList.remove("hidden");
      loadSessionTree();
    } else if (name === "settings") {
      renderSettingsModal();
      $("settings-modal").classList.remove("hidden");
      $("btn-settings").setAttribute("aria-expanded", "true");
    }
  }

  // =========================================================================
  // RENDERING
  // =========================================================================

  function renderAll() {
    renderHeader();
    renderSidebar();
    renderMainStage();
    renderDiffDrawer();
  }

  function renderHeader() {
    const sess = activeSession();
    const dot = $("hdr-status-dot");
    dot.className = "status-dot " + (sess ? sess.status : "idle");
    $("hdr-session-name").textContent = sess ? sess.name : "No active session";
    $("hdr-workspace-name").textContent = state.workspace.name || "Workspace";
    $("btn-workspace").title = state.workspace.path || "Choose workspace folder";

    const cacheDot = $("hdr-cache-dot");
    cacheDot.className = "status-dot " + (sess && sess.cache_warm ? "done" : "idle");
    $("hdr-cache-label").textContent = sess ? sess.cache_label : "Cold · 0%";

    const diffBtn = $("btn-diff");
    diffBtn.setAttribute("aria-pressed", String(state.diffOpen));
    diffBtn.classList.toggle("active", state.diffOpen);
    if (sess && sess.diff && (sess.diff.added || sess.diff.removed)) {
      $("hdr-diff-label").textContent = `+${sess.diff.added} -${sess.diff.removed}`;
    } else if (state.diffData && state.diffData.is_git_repo && (state.diffData.total_added || state.diffData.total_removed)) {
      $("hdr-diff-label").textContent = `+${state.diffData.total_added} -${state.diffData.total_removed}`;
    } else {
      $("hdr-diff-label").textContent = "Diff";
    }

    $("hdr-model-label").textContent = sess ? sess.model_label : "Qwen 3.8";

    const authDot = $("hdr-auth-dot");
    authDot.className = "status-dot " + (state.auth.has_any_configured_key ? "done" : "blocked");

    $("btn-sidebar").setAttribute("aria-expanded", String(state.sidebarOpen));
  }

  function renderSidebar() {
    const sidebar = $("sidebar");
    const scrim = $("sidebar-scrim");
    sidebar.classList.toggle("collapsed", !state.sidebarOpen);
    const narrow = window.innerWidth <= 820;
    scrim.classList.toggle("hidden", !(narrow && state.sidebarOpen));

    $("btn-board").setAttribute("aria-pressed", String(state.boardOpen));
    $("btn-board").classList.toggle("active", state.boardOpen);

    $("sidebar-ws-path").textContent = state.workspace.path || "Select folder...";
    $("app-version").textContent = "v" + state.version;

    const dDot = $("daemon-dot");
    const dLbl = $("daemon-label");
    if (state.daemonError) {
      dDot.className = "status-dot error";
      dLbl.textContent = "Engine error";
      dLbl.title = state.daemonError;
    } else if (state.daemonOk) {
      dDot.className = "status-dot done";
      dLbl.textContent = "Engine ready";
      dLbl.title = "Embedded saint-harness-server running";
    } else {
      dDot.className = "status-dot working";
      dLbl.textContent = "Starting engine...";
    }

    const listEl = $("session-list");
    listEl.textContent = "";

    if (state.daemonError) {
      const box = document.createElement("div");
      box.className = "sidebar-state-msg";
      box.textContent = "Could not connect to embedded engine: " + state.daemonError;
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "secondary-sm-btn";
      retry.style.marginTop = "8px";
      retry.textContent = "Retry";
      retry.addEventListener("click", bootstrap);
      box.appendChild(document.createElement("br"));
      box.appendChild(retry);
      listEl.appendChild(box);
      return;
    }

    if (!state.sessions.length) {
      const empty = document.createElement("div");
      empty.className = "sidebar-state-msg";
      empty.textContent = "No sessions yet. Click + New session to start.";
      listEl.appendChild(empty);
      return;
    }

    for (const grp of STATUS_GROUPS) {
      const items = state.sessions.filter((s) => s.status === grp.key);
      if (!items.length) continue;

      const label = document.createElement("div");
      label.className = "session-group-label";
      label.textContent = `${grp.label} (${items.length})`;
      listEl.appendChild(label);

      for (const s of items) {
        const row = document.createElement("div");
        row.className = "session-row" + (s.id === state.activeSessionId ? " active" : "");

        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "session-item-btn";
        btn.setAttribute("aria-current", s.id === state.activeSessionId ? "true" : "false");
        btn.addEventListener("click", () => {
          state.activeSessionId = s.id;
          state.boardOpen = false;
          if (window.innerWidth <= 820) state.sidebarOpen = false;
          renderAll();
          if (state.diffOpen) refreshDiff();
        });

        const dot = document.createElement("span");
        dot.className = "status-dot " + s.status;
        dot.setAttribute("aria-hidden", "true");

        const main = document.createElement("span");
        main.className = "session-item-main";

        const nameEl = document.createElement("span");
        nameEl.className = "session-item-name";
        nameEl.textContent = s.name || "session";

        const subEl = document.createElement("span");
        subEl.className = "session-item-sub " + (s.subtitle === "error" ? "error" : s.status);
        subEl.textContent = `${s.model_label} · ${s.subtitle || s.status}`;

        main.append(nameEl, subEl);
        btn.append(dot, main);

        const delBtn = document.createElement("button");
        delBtn.type = "button";
        delBtn.className = "session-del-btn";
        delBtn.setAttribute("aria-label", `Delete session ${s.name}`);
        delBtn.title = "Delete session";
        delBtn.textContent = "×";
        delBtn.addEventListener("click", (ev) => {
          ev.stopPropagation();
          handleDeleteSession(s.id);
        });

        row.append(btn, delBtn);
        listEl.appendChild(row);
      }
    }
  }

  function renderMainStage() {
    $("board-view").classList.toggle("hidden", !state.boardOpen);
    $("session-view").classList.toggle("hidden", state.boardOpen);

    if (state.boardOpen) {
      renderBoard();
      return;
    }

    const sess = activeSession();
    const hasMessages = Boolean(sess && sess.messages && sess.messages.length > 0);
    $("landing-view").classList.toggle("hidden", hasMessages);
    $("transcript").classList.toggle("hidden", !hasMessages);

    if (!hasMessages) {
      $("landing-workspace-meta").textContent = state.workspace.path || state.workspace.name;
      $("landing-auth-banner").classList.toggle("hidden", state.auth.has_any_configured_key);
    } else {
      renderTranscript(sess);
    }

    // Steer banner
    const steerEl = $("steer-banner");
    if (state.steerBanner) {
      steerEl.textContent = state.steerBanner;
      steerEl.classList.remove("hidden");
    } else {
      steerEl.classList.add("hidden");
    }

    // Decision card
    renderDecisionCard(sess);

    // Composer footer state
    const imode = sess ? sess.interaction_mode : "build";
    const modeBtn = $("btn-mode");
    $("mode-label").textContent = imode === "plan" ? "Plan" : "Build";
    modeBtn.classList.toggle("mode-plan", imode === "plan");

    const rmode = sess ? sess.runtime_mode : "approval-required";
    const accessBtn = $("btn-access");
    $("access-label").textContent = formatRuntimeLabel(rmode);
    accessBtn.classList.toggle("access-auto", rmode === "auto");
    accessBtn.classList.toggle("access-bypass", rmode === "full-access");

    const costText = sess && sess.cost_usd > 0 ? ` · $${sess.cost_usd.toFixed(4)}` : "";
    $("ctx-usage").textContent = (sess ? sess.ctx_label : "0 / 110K") + costText;

    const isWorking = Boolean(sess && sess.status === "working");
    $("btn-stop").classList.toggle("hidden", !isWorking);
  }

  function renderFormattedMarkdown(container, rawText) {
    // Safe DOM markdown renderer for fenced code blocks + inline code (zero innerHTML XSS)
    const parts = String(rawText || "").split(/```([a-zA-Z0-9_-]*)\n([\s\S]*?)```/g);
    for (let i = 0; i < parts.length; i += 3) {
      const prose = parts[i];
      if (prose) {
        const paragraphs = prose.split(/\n{2,}/);
        for (const para of paragraphs) {
          if (!para.trim()) continue;
          const p = document.createElement("p");
          const segs = para.split(/(`[^`]+`)/g);
          for (const seg of segs) {
            if (seg.startsWith("`") && seg.endsWith("`") && seg.length > 2) {
              const code = document.createElement("code");
              code.className = "inline-code";
              code.textContent = seg.slice(1, -1);
              p.appendChild(code);
            } else {
              p.appendChild(document.createTextNode(seg));
            }
          }
          container.appendChild(p);
        }
      }
      if (i + 2 < parts.length) {
        const lang = parts[i + 1] || "text";
        const codeBody = parts[i + 2];
        const block = document.createElement("div");
        block.className = "code-block";

        const head = document.createElement("div");
        head.className = "code-block-head";
        const langSpan = document.createElement("span");
        langSpan.textContent = lang;
        const copyBtn = document.createElement("button");
        copyBtn.type = "button";
        copyBtn.className = "tb-btn";
        copyBtn.textContent = "Copy";
        copyBtn.addEventListener("click", () => {
          if (navigator.clipboard) {
            navigator.clipboard.writeText(codeBody).then(() => showToast("Copied code block"));
          }
        });
        head.append(langSpan, copyBtn);

        const pre = document.createElement("pre");
        pre.textContent = codeBody;
        block.append(head, pre);
        container.appendChild(block);
      }
    }
  }

  function renderTranscript(sess) {
    const el = $("transcript");
    const viewport = $("scroll-viewport");
    const wasNearBottom = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight < 96;
    el.textContent = "";

    for (let idx = 0; idx < sess.messages.length; idx++) {
      const item = sess.messages[idx];

      if (item.role === "user") {
        const div = document.createElement("div");
        div.className = "msg-user";
        div.textContent = item.text;
        el.appendChild(div);
      } else if (item.role === "handoff") {
        const div = document.createElement("div");
        const isErr = /^error:/i.test(item.text || "");
        div.className = "msg-handoff" + (isErr ? " is-error" : "");

        const txt = document.createElement("span");
        txt.textContent = item.text;
        div.appendChild(txt);

        if (isErr && /api key|auth login/i.test(item.text)) {
          const fixBtn = document.createElement("button");
          fixBtn.type = "button";
          fixBtn.className = "primary-sm-btn";
          fixBtn.textContent = "Open API Keys";
          fixBtn.addEventListener("click", () => openOverlay("settings", fixBtn));
          div.appendChild(fixBtn);
        }
        el.appendChild(div);
      } else if (item.role === "activity") {
        const wrap = document.createElement("div");
        wrap.className = "msg-activity";

        const toggle = document.createElement("button");
        toggle.type = "button";
        toggle.className = "activity-toggle";
        toggle.setAttribute("aria-expanded", String(Boolean(item.expanded)));
        toggle.textContent = (item.expanded ? "▾ " : "▸ ") + (item.summary || "Activity");
        toggle.addEventListener("click", () => {
          item.expanded = !item.expanded;
          renderTranscript(sess);
        });
        wrap.appendChild(toggle);

        if (item.expanded && Array.isArray(item.tools) && item.tools.length) {
          const list = document.createElement("div");
          list.className = "activity-tools";
          for (const t of item.tools) {
            const row = document.createElement("div");
            row.className = "tool-row";

            const verb = document.createElement("span");
            verb.className = "tool-verb";
            verb.textContent = t.verb;

            const arg = document.createElement("span");
            arg.className = "tool-arg";
            arg.textContent = t.arg;

            const note = document.createElement("span");
            note.className = "tool-note";
            note.textContent = t.note;

            row.append(verb, arg, note);
            list.appendChild(row);
          }
          wrap.appendChild(list);
        }
        el.appendChild(wrap);
      } else if (item.role === "assistant") {
        const wrap = document.createElement("article");
        wrap.className = "msg-assistant";

        const body = document.createElement("div");
        body.className = "msg-assistant-body";
        renderFormattedMarkdown(body, item.text);
        wrap.appendChild(body);

        if (item.meta) {
          const meta = document.createElement("div");
          meta.className = "msg-meta";
          meta.textContent = item.meta;
          wrap.appendChild(meta);
        }
        el.appendChild(wrap);
      }
    }

    if (sess.status === "working") {
      const workingRow = document.createElement("div");
      workingRow.className = "msg-handoff";
      workingRow.textContent = "Working on your request...";
      el.appendChild(workingRow);
    }

    if (wasNearBottom) {
      viewport.scrollTop = viewport.scrollHeight;
    }
  }

  function renderDecisionCard(sess) {
    const slot = $("decision-slot");
    slot.textContent = "";
    if (!sess || !sess.decision) {
      slot.classList.add("hidden");
      return;
    }

    const d = sess.decision;
    slot.classList.remove("hidden");

    const card = document.createElement("div");
    card.className = "decision-card";

    const head = document.createElement("div");
    head.className = "decision-head";

    const title = document.createElement("div");
    title.className = "decision-title";
    title.textContent = d.title || "Approve this action?";
    head.appendChild(title);

    if (d.sandbox) {
      const sb = document.createElement("span");
      sb.className = "sandbox-badge";
      sb.textContent = d.sandbox;
      head.appendChild(sb);
    }
    card.appendChild(head);

    if (d.command) {
      const cmd = document.createElement("pre");
      cmd.className = "decision-cmd";
      cmd.textContent = d.command;
      card.appendChild(cmd);
    }

    const choices = document.createElement("div");
    choices.className = "decision-choices";
    const opts = [
      { n: "1", decision: "once", label: "Approve once", decline: false },
      {
        n: "2",
        decision: "always",
        label: d.pattern ? `Always allow pattern (${d.pattern})` : "Approve for this session",
        decline: false,
      },
      { n: "3", decision: "reject_with_feedback", label: "Decline and guide next step", decline: false },
      { n: "4", decision: "reject", label: "Decline and stop turn", decline: true },
    ];

    for (const o of opts) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "decision-choice-btn" + (o.decline ? " decline" : "");
      btn.addEventListener("click", () => handlePermissionChoice(d.request_id, o.decision, d.pattern));

      const num = document.createElement("span");
      num.className = "choice-num";
      num.textContent = `[${o.n}]`;

      const lbl = document.createElement("span");
      lbl.textContent = o.label;

      btn.append(num, lbl);
      choices.appendChild(btn);
    }

    card.appendChild(choices);
    slot.appendChild(card);
  }

  function renderBoard() {
    const grid = $("board-grid");
    grid.textContent = "";

    for (const grp of STATUS_GROUPS) {
      const items = state.sessions.filter((s) => s.status === grp.key);
      const col = document.createElement("div");
      col.className = "board-col";

      const head = document.createElement("div");
      head.className = "board-col-head";
      const left = document.createElement("span");
      left.className = "board-col-title";
      const dot = document.createElement("span");
      dot.className = "status-dot " + grp.key;
      dot.setAttribute("aria-hidden", "true");
      left.append(dot, document.createTextNode(grp.label));

      const count = document.createElement("span");
      count.className = "mono muted";
      count.textContent = String(items.length);
      head.append(left, count);
      col.appendChild(head);

      if (!items.length) {
        const empty = document.createElement("div");
        empty.className = "board-empty";
        empty.textContent = `No sessions in ${grp.label}.`;
        col.appendChild(empty);
      } else {
        for (const s of items) {
          const card = document.createElement("button");
          card.type = "button";
          card.className = "board-card";
          card.addEventListener("click", () => {
            state.activeSessionId = s.id;
            state.boardOpen = false;
            renderAll();
          });

          const name = document.createElement("span");
          name.className = "board-card-name";
          name.textContent = s.name;

          const meta = document.createElement("span");
          meta.className = "board-card-meta mono";
          meta.textContent = `${s.model_label} · ${s.subtitle || s.status}`;

          card.append(name, meta);
          col.appendChild(card);
        }
      }

      grid.appendChild(col);
    }
  }

  function renderDiffDrawer() {
    const drawer = $("diff-drawer");
    drawer.classList.toggle("hidden", !state.diffOpen);
    if (!state.diffOpen) return;

    const body = $("diff-body");
    body.textContent = "";

    if (state.diffLoading) {
      $("diff-branch").textContent = "";
      const msg = document.createElement("div");
      msg.className = "empty-state";
      msg.textContent = "Loading workspace git diff...";
      body.appendChild(msg);
      return;
    }

    if (!state.diffData) {
      const msg = document.createElement("div");
      msg.className = "empty-state";
      msg.textContent = "Click Refresh to inspect git changes in this workspace.";
      body.appendChild(msg);
      return;
    }

    if (!state.diffData.is_git_repo) {
      $("diff-branch").textContent = "no git repo";
      const msg = document.createElement("div");
      msg.className = "empty-state";
      msg.textContent = `This folder (${state.diffData.cwd}) is not a Git repository.`;
      body.appendChild(msg);
      return;
    }

    $("diff-branch").textContent = `${state.diffData.branch} · +${state.diffData.total_added} -${state.diffData.total_removed}`;

    if (!state.diffData.files.length) {
      const msg = document.createElement("div");
      msg.className = "empty-state";
      msg.textContent = "No uncommitted changes in this workspace yet.";
      body.appendChild(msg);
      return;
    }

    for (const f of state.diffData.files) {
      const card = document.createElement("div");
      card.className = "diff-file-card";

      const head = document.createElement("div");
      head.className = "diff-file-head";
      const p = document.createElement("span");
      p.className = "diff-file-path";
      p.textContent = f.path;
      p.title = f.path;

      const stats = document.createElement("span");
      if (f.status === "untracked") {
        stats.className = "muted";
        stats.textContent = "untracked";
      } else {
        const a = document.createElement("span");
        a.className = "diff-stat-add";
        a.textContent = `+${f.added} `;
        const d = document.createElement("span");
        d.className = "diff-stat-del";
        d.textContent = `-${f.removed}`;
        stats.append(a, d);
      }
      head.append(p, stats);
      card.appendChild(head);

      if (f.hunks && f.hunks.length) {
        const hunks = document.createElement("div");
        hunks.className = "diff-hunks";
        for (const line of f.hunks) {
          const row = document.createElement("div");
          row.className =
            "diff-line" +
            (line.startsWith("+") ? " add" : line.startsWith("-") ? " del" : line.startsWith("@@") ? " hunk" : "");
          row.textContent = line;
          hunks.appendChild(row);
        }
        card.appendChild(hunks);
      }

      body.appendChild(card);
    }
  }

  function renderModelPopover() {
    const body = $("model-popover-body");
    body.textContent = "";
    const sess = activeSession();
    const activeKey = sess ? sess.model_key : "qwen";

    for (const leaf of state.models.leaves || []) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "pop-row" + (leaf.key === activeKey ? " selected" : "");
      row.addEventListener("click", () => {
        const profile = leaf.key === "qwen4" ? "qwen-4-upcoming" : "qwen-3.8-27b";
        selectModel(profile, leaf.name, leaf.key, null);
      });

      const left = document.createElement("span");
      left.className = "pop-row-left";
      left.textContent = (leaf.key === activeKey ? "✓ " : "  ") + leaf.label;

      const meta = document.createElement("span");
      meta.className = "pop-row-meta";
      meta.textContent = leaf.edit_mode ? `${leaf.edit_mode}` : leaf.selector;

      row.append(left, meta);
      body.appendChild(row);
    }

    const div = document.createElement("div");
    div.className = "pop-divider";
    body.appendChild(div);

    for (const prov of state.models.providers || []) {
      const expanded = state.expandedProviders.has(prov.key);
      const headBtn = document.createElement("button");
      headBtn.type = "button";
      headBtn.className = "pop-row";
      headBtn.setAttribute("aria-expanded", String(expanded));
      headBtn.addEventListener("click", () => {
        if (expanded) state.expandedProviders.delete(prov.key);
        else state.expandedProviders.add(prov.key);
        renderModelPopover();
      });

      const left = document.createElement("span");
      left.className = "pop-row-left";
      left.textContent = (expanded ? "▾ " : "▸ ") + prov.label;

      const meta = document.createElement("span");
      meta.className = "pop-row-meta";
      meta.textContent = prov.tag;

      headBtn.append(left, meta);
      body.appendChild(headBtn);

      if (expanded) {
        for (const child of prov.children || []) {
          const cBtn = document.createElement("button");
          cBtn.type = "button";
          cBtn.className = "pop-row child" + (child.key === activeKey ? " selected" : "");
          cBtn.addEventListener("click", () => {
            const profile = prov.key === "gemini" ? "gemini-flagship" : "claude-flagship";
            const modelOverride = child.key !== prov.key ? child.selector : null;
            selectModel(profile, child.name, child.key, modelOverride);
          });

          const cLeft = document.createElement("span");
          cLeft.className = "pop-row-left";
          cLeft.textContent = (child.key === activeKey ? "✓ " : "  ") + child.label;

          const cMeta = document.createElement("span");
          cMeta.className = "pop-row-meta";
          cMeta.textContent = child.selector;

          cBtn.append(cLeft, cMeta);
          body.appendChild(cBtn);
        }
      }
    }
  }

  function renderAccessPopover() {
    const body = $("access-popover-body");
    body.textContent = "";
    const sess = activeSession();
    const cur = sess ? sess.runtime_mode : "approval-required";

    for (const opt of ACCESS_MODES) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "pop-row" + (opt.value === cur ? " selected" : "");
      row.addEventListener("click", () => {
        if (sess) {
          sess.runtime_mode = opt.value;
        }
        closeOverlays();
        renderAll();
      });

      const left = document.createElement("span");
      left.className = "pop-row-left";
      left.textContent = (opt.value === cur ? "✓ " : "  ") + opt.label;

      const meta = document.createElement("span");
      meta.className = "pop-row-meta" + (opt.warn ? " warn" : "");
      meta.textContent = opt.meta;

      row.append(left, meta);
      body.appendChild(row);
    }
  }

  function renderCachePopover() {
    const body = $("cache-popover-body");
    body.textContent = "";
    const sess = activeSession();
    const rows = [
      ["Cache status", sess ? sess.cache_label : "Cold · 0%"],
      ["Context usage", sess ? sess.ctx_label : "0 / 110K"],
      ["Session cost", sess ? `$${(sess.cost_usd || 0).toFixed(4)}` : "$0.0000"],
      ["Active profile", sess ? sess.model_profile : state.defaultProfile],
    ];
    for (const [k, v] of rows) {
      const r = document.createElement("div");
      r.className = "kv-row";
      const kEl = document.createElement("span");
      kEl.className = "muted";
      kEl.textContent = k;
      const vEl = document.createElement("span");
      vEl.className = "mono";
      vEl.textContent = v;
      r.append(kEl, vEl);
      body.appendChild(r);
    }
  }

  function renderSettingsModal() {
    $("settings-path-meta").textContent = `Stored locally in ${state.auth.settings_path} (chmod 0600).`;
    const list = $("provider-list");
    list.textContent = "";

    const providers = state.auth.providers || [];
    if (!providers.length) {
      const empty = document.createElement("div");
      empty.className = "empty-state";
      empty.textContent = "Provider list unavailable.";
      list.appendChild(empty);
      return;
    }

    let selected = providers.find((p) => p.id === state.selectedProviderId) || providers[0];
    state.selectedProviderId = selected.id;

    for (const p of providers) {
      const tab = document.createElement("button");
      tab.type = "button";
      tab.role = "tab";
      tab.className = "provider-tab" + (p.id === selected.id ? " active" : "");
      tab.setAttribute("aria-selected", String(p.id === selected.id));
      tab.addEventListener("click", () => {
        state.selectedProviderId = p.id;
        $("pf-feedback").classList.add("hidden");
        renderSettingsModal();
      });

      const name = document.createElement("span");
      name.textContent = p.label;

      const dot = document.createElement("span");
      dot.className = "status-dot " + (p.configured ? "done" : p.is_local ? "idle" : "blocked");
      dot.setAttribute("aria-hidden", "true");

      tab.append(name, dot);
      list.appendChild(tab);
    }

    $("pf-title").textContent = selected.label;
    const badge = $("pf-status-badge");
    if (selected.configured) {
      badge.textContent = `Configured (${selected.source})`;
      badge.className = "status-pill ok";
    } else if (selected.is_local) {
      badge.textContent = "Local (no key required)";
      badge.className = "status-pill";
    } else {
      badge.textContent = "Not configured";
      badge.className = "status-pill";
    }

    $("pf-env-name").textContent = `(${selected.env_key})`;
    $("pf-api-key").value = "";
    $("pf-preview-hint").textContent = selected.configured
      ? `Saved key preview: ${selected.preview || "***"}`
      : "No API key saved for this provider yet.";
    $("pf-base-url").value = selected.base_url || selected.default_base_url;
    $("pf-base-url").placeholder = selected.default_base_url;
    $("pf-set-default").checked = state.auth.default_profile === selected.default_profile;
    $("btn-pf-remove").disabled = !selected.configured;
  }

  function renderSlashPalette() {
    const pal = $("slash-palette");
    if (!state.slashMatches.length) {
      pal.classList.add("hidden");
      return;
    }
    pal.classList.remove("hidden");
    pal.textContent = "";
    state.slashMatches.forEach((item, idx) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.role = "option";
      btn.className = "palette-row" + (idx === state.slashIndex ? " selected" : "");
      btn.setAttribute("aria-selected", String(idx === state.slashIndex));
      btn.addEventListener("click", () => {
        runSlashCommand(item.cmd);
      });

      const cmd = document.createElement("span");
      cmd.className = "palette-cmd";
      cmd.textContent = item.cmd;

      const desc = document.createElement("span");
      desc.className = "palette-desc";
      desc.textContent = item.desc;

      btn.append(cmd, desc);
      pal.appendChild(btn);
    });
  }

  // =========================================================================
  // ACTIONS & IPC
  // =========================================================================

  function selectModel(profile, label, key, modelOverride) {
    const sess = activeSession();
    if (sess) {
      sess.model_profile = profile;
      sess.model_label = label;
      sess.model_key = key;
      sess._modelOverride = modelOverride;
      if (sess.messages && sess.messages.length) {
        sess.messages.push({
          role: "handoff",
          text: `Switched model to ${label} (${profile}).`,
        });
      }
    }
    closeOverlays();
    renderAll();
  }

  async function bootstrap() {
    state.daemonError = null;
    renderSidebar();
    try {
      const res = await invoke("desktop_bootstrap");
      state.version = res.version;
      state.workspace = res.workspace;
      state.defaultProfile = res.default_profile;
      state.sessions = res.sessions || [];
      state.models = res.models || { leaves: [], providers: [] };
      state.auth = res.auth || state.auth;
      state.daemonOk = true;
      if (state.sessions.length && !state.sessions.some((s) => s.id === state.activeSessionId)) {
        state.activeSessionId = state.sessions[0].id;
      }
      renderAll();
    } catch (err) {
      state.daemonOk = false;
      state.daemonError = String(err);
      renderAll();
    }
  }

  async function handleNewSession(customCwd = null) {
    try {
      const created = await invoke("create_session", {
        cwd: customCwd,
        title: "new session",
        modelProfile: state.auth.default_profile || state.defaultProfile,
      });
      if (!state.sessions.some((s) => s.id === created.session.id)) {
        state.sessions.unshift(created.session);
      }
      state.activeSessionId = created.session.id;
      state.boardOpen = false;
      if (window.innerWidth <= 820) state.sidebarOpen = false;
      renderAll();
      $("composer-input").focus();
    } catch (err) {
      showToast("Failed to create session: " + String(err));
    }
  }

  async function handleDeleteSession(sessionId) {
    try {
      await invoke("delete_session", { sessionId });
      state.sessions = state.sessions.filter((s) => s.id !== sessionId);
      if (state.activeSessionId === sessionId) {
        state.activeSessionId = state.sessions.length ? state.sessions[0].id : null;
      }
      if (!state.sessions.length) {
        await handleNewSession();
        return;
      }
      renderAll();
    } catch (err) {
      showToast("Failed to delete session: " + String(err));
    }
  }

  async function handleOpenWorkspace() {
    try {
      const chosen = await pickFolderNative(state.workspace.path);
      if (!chosen || typeof chosen !== "string") return;
      const info = await invoke("set_workspace", { path: chosen });
      state.workspace = info;
      await handleNewSession(info.path);
      if (state.diffOpen) await refreshDiff();
      showToast(`Opened workspace ${info.name}`);
    } catch (err) {
      showToast("Failed to open folder: " + String(err));
    }
  }

  async function refreshDiff() {
    state.diffLoading = true;
    renderDiffDrawer();
    try {
      state.diffData = await invoke("get_workspace_diff", {
        sessionId: state.activeSessionId,
      });
    } catch (err) {
      showToast("Diff refresh failed: " + String(err));
    } finally {
      state.diffLoading = false;
      renderAll();
    }
  }

  async function loadSessionTree() {
    const body = $("tree-modal-body");
    body.textContent = "Loading session tree...";
    const sess = activeSession();
    if (!sess) return;
    try {
      const nodes = await invoke("get_session_tree", { sessionId: sess.id });
      body.textContent = "";
      if (!nodes.length) {
        const empty = document.createElement("div");
        empty.className = "empty-state";
        empty.textContent = "No turns recorded in this session yet.";
        body.appendChild(empty);
        return;
      }
      for (const n of nodes) {
        const row = document.createElement("div");
        row.className = "tree-node-row";
        const info = document.createElement("div");
        info.className = "tree-node-info";
        const role = document.createElement("span");
        role.className = "tree-node-role";
        role.textContent = `#${n.seq} · ${n.role}` + (n.is_head ? " (HEAD)" : "");
        const prev = document.createElement("span");
        prev.className = "tree-node-preview";
        prev.textContent = n.preview || "(empty)";
        info.append(role, prev);

        const forkBtn = document.createElement("button");
        forkBtn.type = "button";
        forkBtn.className = "secondary-sm-btn";
        forkBtn.textContent = "Fork here";
        forkBtn.addEventListener("click", () => handleForkSession(n.id));

        row.append(info, forkBtn);
        body.appendChild(row);
      }
    } catch (err) {
      body.textContent = "Could not load branch tree: " + String(err);
    }
  }

  async function handleForkSession(throughMsgId) {
    const sess = activeSession();
    if (!sess) return;
    try {
      const forked = await invoke("fork_session", {
        sessionId: sess.id,
        throughMsgId: throughMsgId || null,
      });
      state.sessions.unshift(forked.session);
      state.activeSessionId = forked.session.id;
      closeOverlays();
      renderAll();
      showToast("Forked into new session");
    } catch (err) {
      showToast("Fork failed: " + String(err));
    }
  }

  function runSlashCommand(cmd) {
    const input = $("composer-input");
    input.value = "";
    autosizeComposer();
    state.slashMatches = [];
    renderSlashPalette();

    const sess = activeSession();
    switch (cmd) {
      case "/plan":
        if (sess) {
          sess.interaction_mode = "plan";
          renderAll();
          showToast("Switched to read-only Plan mode");
        }
        break;
      case "/auto":
        if (sess) {
          sess.runtime_mode = "auto";
          renderAll();
          showToast("Auto mode enabled");
        }
        break;
      case "/bypass":
        if (sess) {
          sess.runtime_mode = "full-access";
          renderAll();
          showToast("Bypass permissions enabled");
        }
        break;
      case "/supervised":
        if (sess) {
          sess.runtime_mode = "approval-required";
          renderAll();
          showToast("Ask for approval enabled");
        }
        break;
      case "/diff":
        state.diffOpen = !state.diffOpen;
        renderAll();
        if (state.diffOpen) refreshDiff();
        break;
      case "/btw":
        openOverlay("btw", $("composer-input"));
        break;
      case "/tree":
        openOverlay("tree", $("composer-input"));
        break;
      case "/board":
        state.boardOpen = !state.boardOpen;
        renderAll();
        break;
      case "/model":
        openOverlay("model", $("btn-model"));
        break;
      case "/compact":
        if (sess) {
          invoke("compact_session", { sessionId: sess.id })
            .then((r) => {
              sess.messages.push({
                role: "handoff",
                text: `Tier 1 compaction: ${r.before_tokens} -> ${r.after_tokens} tokens (${r.spilled_blocks} spilled blocks).`,
              });
              renderAll();
            })
            .catch((e) => showToast("Compaction failed: " + String(e)));
        }
        break;
      case "/new":
        handleNewSession();
        break;
      case "/settings":
        openOverlay("settings", $("btn-settings"));
        break;
      case "/theme":
        applyTheme(state.theme === "dark" ? "light" : "dark");
        break;
      default:
        break;
    }
  }

  async function handleSubmitPrompt(overrideText = null) {
    const input = $("composer-input");
    const raw = overrideText !== null ? overrideText : input.value;
    const text = raw.trim();
    if (!text) return;

    if (text.startsWith("/")) {
      const cmd = text.split(/\s+/)[0].toLowerCase();
      if (SLASH_COMMANDS.some((c) => c.cmd === cmd)) {
        runSlashCommand(cmd);
        return;
      }
    }

    if (overrideText === null) {
      input.value = "";
      autosizeComposer();
      state.slashMatches = [];
      renderSlashPalette();
    }

    let sess = activeSession();
    if (!sess) {
      await handleNewSession();
      sess = activeSession();
      if (!sess) return;
    }

    const isWorking = sess.status === "working";
    if (isWorking) {
      state.steerBanner = `Steer queued: "${text}"`;
    } else {
      if (sess.status === "idle" && (!sess.messages || !sess.messages.length)) {
        sess.name = text.slice(0, 36);
      }
      sess.messages = sess.messages || [];
      sess.messages.push({ role: "user", text });
      sess.status = "working";
      sess.subtitle = "working...";
    }
    renderAll();

    try {
      await invoke("send_prompt", {
        sessionId: sess.id,
        text,
        delivery: isWorking ? "steer" : "queue",
        profileOverride: sess.model_profile || state.defaultProfile,
        modelOverride: sess._modelOverride || null,
        interactionMode: sess.interaction_mode || "build",
        runtimeMode: sess.runtime_mode || "approval-required",
      });
    } catch (err) {
      sess.status = "done";
      sess.subtitle = "error";
      sess.messages.push({
        role: "handoff",
        text: "Error: " + String(err),
      });
      renderAll();
    }
  }

  async function handlePermissionChoice(requestId, decision, pattern) {
    const sess = activeSession();
    try {
      await invoke("reply_permission", {
        requestId,
        decision,
        pattern: decision === "always" ? pattern || null : null,
        feedback: decision === "reject_with_feedback" ? "Please adjust the command before running." : null,
      });
      if (sess) {
        sess.decision = null;
        sess.status = decision === "reject" ? "done" : "working";
        sess.subtitle = decision === "reject" ? "stopped" : "working...";
        renderAll();
      }
    } catch (err) {
      showToast("Permission reply failed: " + String(err));
    }
  }

  async function handleInterrupt() {
    const sess = activeSession();
    if (!sess) return;
    try {
      await invoke("interrupt_session", { sessionId: sess.id });
      sess.status = "done";
      sess.subtitle = "interrupted";
      state.steerBanner = null;
      renderAll();
    } catch (err) {
      showToast("Interrupt failed: " + String(err));
    }
  }

  // =========================================================================
  // LIVE SERVER EVENT STREAM (from Tauri app.emit("saint-event", wire))
  // =========================================================================

  function applyWireEvent(wire) {
    const sid = wire.session_id;
    let sess = state.sessions.find((s) => s.id === sid);
    const ev = wire.event_type;
    const p = wire.payload || {};

    if (ev === "session.created") {
      if (!sess) {
        state.sessions.unshift({
          id: sid,
          name: p.title || "new session",
          status: "idle",
          model_profile: p.model_profile || state.defaultProfile,
          model_label: p.model_profile || state.defaultProfile,
          model_key: "qwen",
          ctx_label: "0 / 110K",
          diff: null,
          subtitle: "main",
          cache_warm: false,
          cache_label: "Cold · 0%",
          cost_usd: 0,
          runtime_mode: "approval-required",
          interaction_mode: "build",
          decision: null,
          messages: [],
        });
      }
      renderAll();
      return;
    }

    if (!sess) return;
    sess.messages = sess.messages || [];

    if (ev === "step.started") {
      sess.status = "working";
      sess.subtitle = "working...";
    } else if (ev === "text.delta") {
      const delta = p.delta || "";
      if (delta) {
        const last = sess.messages[sess.messages.length - 1];
        if (last && last.role === "assistant") {
          last.text += delta;
        } else {
          sess.messages.push({ role: "assistant", text: delta, meta: null });
        }
      }
    } else if (ev === "tool.call.started") {
      const name = p.name || "tool";
      const argsStr = p.args ? JSON.stringify(p.args).slice(0, 80) : "";
      let act = sess.messages[sess.messages.length - 1];
      if (!act || act.role !== "activity") {
        act = { role: "activity", summary: "Running tools...", working: true, expanded: false, tools: [] };
        sess.messages.push(act);
      }
      act.tools.push({ verb: name, arg: argsStr, note: "running..." });
      act.summary = `Working · ${act.tools.length} tool${act.tools.length === 1 ? "" : "s"}`;
    } else if (ev === "tool.call.finished") {
      const name = p.name || "tool";
      const ok = p.outcome === "ok" || !p.error;
      const act = [...sess.messages].reverse().find((m) => m.role === "activity");
      if (act && act.tools.length) {
        const matching = [...act.tools].reverse().find((t) => t.verb === name && t.note === "running...");
        if (matching) matching.note = ok ? "done" : "error";
        act.summary = `Worked · ${act.tools.length} tool${act.tools.length === 1 ? "" : "s"}`;
      }
    } else if (ev === "permission.requested") {
      const req = p.request || p;
      sess.status = "blocked";
      sess.subtitle = "1 approval";
      sess.decision = {
        request_id: req.request_id || "",
        kind: "shell",
        title: req.title || "Approve this command?",
        command: req.resource || "",
        sandbox: req.sandbox_summary || "WorkspaceWrite",
        pattern: req.suggested_pattern || null,
      };
    } else if (ev === "turn.diff.updated") {
      sess.diff = {
        added: Number(p.lines_added || 0),
        removed: Number(p.lines_removed || 0),
      };
      if (state.diffOpen && sid === state.activeSessionId) {
        refreshDiff();
      }
    } else if (ev === "step.ended") {
      sess.status = "done";
      sess.subtitle = "done";
      state.steerBanner = null;
      const usage = p.usage || {};
      const inp = Number(usage.input_tokens || 0);
      const cached = Number(usage.cache_read_input_tokens || 0);
      const cost = Number(p.cost_usd || 0);
      sess.cost_usd = (sess.cost_usd || 0) + cost;
      if (inp > 0) {
        const pct = Math.round((cached * 100) / inp);
        sess.cache_warm = cached > 0;
        sess.cache_label = `${cached > 0 ? "Warm" : "Cold"} · ${pct}%`;
        sess.ctx_label = `${(inp / 1000).toFixed(1)}K / 110K`;
      }
    } else if (ev === "step.failed") {
      sess.status = "done";
      sess.subtitle = "error";
      state.steerBanner = null;
      sess.messages.push({
        role: "handoff",
        text: "Error: " + (p.error || "step failed"),
      });
    }

    renderAll();
  }

  // =========================================================================
  // COMPOSER & EVENT WIRING
  // =========================================================================

  function autosizeComposer() {
    const el = $("composer-input");
    el.style.height = "auto";
    el.style.height = Math.min(180, Math.max(24, el.scrollHeight)) + "px";
  }

  function updateSlashPalette() {
    const val = $("composer-input").value;
    if (val.startsWith("/") && !val.includes(" ") && !val.includes("\n")) {
      const q = val.toLowerCase();
      state.slashMatches = SLASH_COMMANDS.filter(
        (c) => c.cmd.includes(q) || c.desc.toLowerCase().includes(q)
      );
      state.slashIndex = 0;
    } else {
      state.slashMatches = [];
      state.slashIndex = 0;
    }
    renderSlashPalette();
  }

  function bindEvents() {
    $("btn-sidebar").addEventListener("click", () => {
      state.sidebarOpen = !state.sidebarOpen;
      renderSidebar();
      renderHeader();
    });

    $("sidebar-scrim").addEventListener("click", () => {
      state.sidebarOpen = false;
      renderSidebar();
      renderHeader();
    });

    $("btn-new-session").addEventListener("click", () => handleNewSession());
    $("btn-board").addEventListener("click", () => {
      state.boardOpen = !state.boardOpen;
      renderAll();
    });
    $("btn-board-close").addEventListener("click", () => {
      state.boardOpen = false;
      renderAll();
    });

    $("btn-workspace").addEventListener("click", handleOpenWorkspace);
    $("btn-sidebar-workspace").addEventListener("click", handleOpenWorkspace);

    $("btn-diff").addEventListener("click", () => {
      state.diffOpen = !state.diffOpen;
      renderAll();
      if (state.diffOpen) refreshDiff();
    });
    $("btn-diff-close").addEventListener("click", () => {
      state.diffOpen = false;
      renderAll();
    });
    $("btn-diff-refresh").addEventListener("click", refreshDiff);

    $("btn-model").addEventListener("click", (e) => openOverlay("model", e.currentTarget));
    $("btn-access").addEventListener("click", (e) => openOverlay("access", e.currentTarget));
    $("btn-cache").addEventListener("click", (e) => openOverlay("cache", e.currentTarget));
    $("btn-settings").addEventListener("click", (e) => openOverlay("settings", e.currentTarget));
    $("btn-landing-auth").addEventListener("click", (e) => openOverlay("settings", e.currentTarget));
    $("btn-theme").addEventListener("click", () => {
      applyTheme(state.theme === "dark" ? "light" : "dark");
    });

    $("btn-mode").addEventListener("click", () => {
      const sess = activeSession();
      if (!sess) return;
      sess.interaction_mode = sess.interaction_mode === "plan" ? "build" : "plan";
      renderAll();
    });

    $("btn-send").addEventListener("click", () => handleSubmitPrompt());
    $("btn-stop").addEventListener("click", handleInterrupt);

    document.querySelectorAll(".starter-chip").forEach((chip) => {
      chip.addEventListener("click", () => {
        const prompt = chip.getAttribute("data-prompt");
        if (prompt) handleSubmitPrompt(prompt);
      });
    });

    document.querySelectorAll("[data-close-overlay]").forEach((btn) => {
      btn.addEventListener("click", closeOverlays);
    });

    // Click backdrop to close modals
    ["tree-modal", "settings-modal"].forEach((id) => {
      $(id).addEventListener("click", (ev) => {
        if (ev.target === $(id)) closeOverlays();
      });
    });

    $("btn-fork-head").addEventListener("click", () => handleForkSession(null));

    // /btw sidechat form
    $("btw-form").addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const q = $("btw-input").value.trim();
      const sess = activeSession();
      if (!q || !sess) return;
      $("btw-input").value = "";
      $("btw-output").textContent = "Querying read-only sidechat...";
      try {
        const res = await invoke("send_btw", { sessionId: sess.id, question: q });
        $("btw-output").textContent = res.answer;
      } catch (err) {
        $("btw-output").textContent = "Sidechat error: " + String(err);
      }
    });

    // Settings / API Keys form
    $("provider-form").addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const fb = $("pf-feedback");
      fb.classList.add("hidden");
      try {
        state.auth = await invoke("save_provider_auth", {
          req: {
            provider: state.selectedProviderId,
            api_key: $("pf-api-key").value,
            base_url: $("pf-base-url").value,
            set_default: $("pf-set-default").checked,
          },
        });
        state.defaultProfile = state.auth.default_profile;
        fb.textContent = "Saved to ~/.saint-harness/settings.json (chmod 0600).";
        fb.className = "form-feedback ok";
        renderSettingsModal();
        renderAll();
      } catch (err) {
        fb.textContent = "Save failed: " + String(err);
        fb.className = "form-feedback err";
      }
    });

    $("btn-pf-test").addEventListener("click", async () => {
      const fb = $("pf-feedback");
      fb.textContent = "Testing connection...";
      fb.className = "form-feedback";
      try {
        const res = await invoke("test_provider_connection", {
          provider: state.selectedProviderId,
          apiKey: $("pf-api-key").value || null,
          baseUrl: $("pf-base-url").value || null,
        });
        fb.textContent = res.message;
        fb.className = "form-feedback " + (res.ok ? "ok" : "err");
      } catch (err) {
        fb.textContent = "Test error: " + String(err);
        fb.className = "form-feedback err";
      }
    });

    $("btn-pf-remove").addEventListener("click", async () => {
      const fb = $("pf-feedback");
      try {
        state.auth = await invoke("remove_provider_auth", {
          provider: state.selectedProviderId,
        });
        fb.textContent = "Removed saved key from settings.json.";
        fb.className = "form-feedback ok";
        renderSettingsModal();
        renderAll();
      } catch (err) {
        fb.textContent = "Remove failed: " + String(err);
        fb.className = "form-feedback err";
      }
    });

    // Composer keyboard interactions
    const input = $("composer-input");
    input.addEventListener("input", () => {
      autosizeComposer();
      updateSlashPalette();
    });

    input.addEventListener("keydown", (ev) => {
      if (state.slashMatches.length) {
        if (ev.key === "ArrowDown") {
          ev.preventDefault();
          state.slashIndex = (state.slashIndex + 1) % state.slashMatches.length;
          renderSlashPalette();
          return;
        }
        if (ev.key === "ArrowUp") {
          ev.preventDefault();
          state.slashIndex =
            (state.slashIndex - 1 + state.slashMatches.length) % state.slashMatches.length;
          renderSlashPalette();
          return;
        }
        if (ev.key === "Enter" || ev.key === "Tab") {
          ev.preventDefault();
          runSlashCommand(state.slashMatches[state.slashIndex].cmd);
          return;
        }
      }

      if (ev.key === "Enter" && !ev.shiftKey) {
        ev.preventDefault();
        handleSubmitPrompt();
      }
    });

    // Global keyboard shortcuts matching SaintHarness contract
    window.addEventListener("keydown", (ev) => {
      const mod = ev.ctrlKey || ev.metaKey;

      if (ev.key === "Escape") {
        if (state.slashMatches.length) {
          state.slashMatches = [];
          renderSlashPalette();
          return;
        }
        if (state.activeOverlay) {
          closeOverlays();
          return;
        }
        if (state.boardOpen) {
          state.boardOpen = false;
          renderAll();
          return;
        }
        if (state.diffOpen) {
          state.diffOpen = false;
          renderAll();
          return;
        }
        if (window.innerWidth <= 820 && state.sidebarOpen) {
          state.sidebarOpen = false;
          renderSidebar();
          return;
        }
        return;
      }

      if (mod && ev.key.toLowerCase() === "b") {
        ev.preventDefault();
        state.sidebarOpen = !state.sidebarOpen;
        renderSidebar();
        renderHeader();
      } else if (mod && ev.key.toLowerCase() === "d") {
        ev.preventDefault();
        state.diffOpen = !state.diffOpen;
        renderAll();
        if (state.diffOpen) refreshDiff();
      } else if (mod && ev.key.toLowerCase() === "g") {
        ev.preventDefault();
        state.boardOpen = !state.boardOpen;
        renderAll();
      } else if (mod && ev.key.toLowerCase() === "n") {
        ev.preventDefault();
        handleNewSession();
      } else if (mod && ev.key.toLowerCase() === "t") {
        ev.preventDefault();
        openOverlay("tree", $("composer-input"));
      } else if (mod && ev.key === ",") {
        ev.preventDefault();
        openOverlay("settings", $("btn-settings"));
      } else if (mod && ev.altKey && ev.key.toLowerCase() === "s") {
        ev.preventDefault();
        openOverlay("btw", $("composer-input"));
      } else if (ev.altKey && ev.key.toLowerCase() === "m") {
        ev.preventDefault();
        openOverlay("model", $("btn-model"));
      } else if (ev.altKey && ev.key.toLowerCase() === "a") {
        ev.preventDefault();
        openOverlay("access", $("btn-access"));
      } else if (ev.altKey && ev.key.toLowerCase() === "t") {
        ev.preventDefault();
        applyTheme(state.theme === "dark" ? "light" : "dark");
      } else if (ev.shiftKey && ev.key === "Tab") {
        ev.preventDefault();
        const sess = activeSession();
        if (sess) {
          sess.interaction_mode = sess.interaction_mode === "plan" ? "build" : "plan";
          renderAll();
        }
      }
    });

    let wasWide = window.innerWidth > 820;
    window.addEventListener("resize", () => {
      const nowWide = window.innerWidth > 820;
      if (wasWide && !nowWide) {
        state.sidebarOpen = false;
      }
      wasWide = nowWide;
      renderSidebar();
      renderHeader();
    });
  }

  async function init() {
    let savedTheme = "dark";
    try {
      savedTheme = localStorage.getItem("saint-harness-desktop-theme") || "dark";
    } catch (_) {}
    applyTheme(savedTheme);
    bindEvents();

    if (window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.listen) {
      await window.__TAURI__.event.listen("saint-event", (event) => {
        if (event && event.payload) {
          applyWireEvent(event.payload);
        }
      });
    }

    await bootstrap();
  }

  document.addEventListener("DOMContentLoaded", init);
})();

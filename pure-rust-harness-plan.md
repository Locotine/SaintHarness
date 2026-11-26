# Master Implementation Plan — `rust-harness` (Pure Rust Coding Agent Harness)

> **Mục tiêu tài liệu:** Bản đặc tả kỹ thuật & kế hoạch thực thi **cấp độ triển khai (Implementation-Grade)** để giao trực tiếp cho Frontier Coding Models xây dựng hoàn chỉnh **`rust-harness`** — một **Coding Agent Harness 100% Pure Rust** độc lập, khởi động tức thì trong 1 file binary duy nhất:
> 1. **Xương sống bền vững & Kỷ luật Prompt Cache:** Event Sourcing trên SQLite WAL (1 writer thread), Doorbell Coordinator, vòng lặp 16 nhánh, crash-recovery tự động, `InstructionState` delta, và khóa cứng byte-exact prefix determinism.
> 2. **Động cơ Tiết kiệm Token & Tăng tốc Pure Rust (V1 + V1.5):** `Hashline` FNV-1a, `imara-diff` (Histogram Diff), `RepoMap` (`tree-sitter` + Personalized PageRank), LSP Symbol Navigation (`symbol_read`/`find_callers`/`replace_symbol`), Delta Re-Read (`read_diff`), `ripgrep` crates + `nucleo-matcher`, Semantic CLI/JSON Condensers + `zstd`, Observation Masking + Compaction 5 tầng, `ast-grep-core` (`ast_edit`), `gix` Pure Rust Git + Shadow Snapshot, `oxc`/`ruff` fast lint & Type Stripping, `tantivy` BM25 indexer.
> 3. **Scope Tri-Model Data-Driven (Không hardcode phiên bản provider):** **Qwen 3.8 27B & Qwen 4 (sắp ra mắt)** + **Claude (flagship mới nhất)** + **Gemini (flagship mới nhất)**. Mọi model ID, năng lực và cửa sổ ngữ cảnh của Claude/Gemini được giải phân giải động qua `ModelResolver` (`catalog.rs` + `ModelProfile` TOML `selector = "latest_flagship"`), không hardcode số phiên bản trong mã Rust.
> 4. **Chính sách Guardrail sạch & Clean-Room tuyệt đối:** Không nhúng kiểm duyệt chủ đề/đạo đức dựng sẵn (`NoopPolicyGuard`); **tuyệt đối không nhắc tên hay trích dẫn bất kỳ dự án/agent/harness tham khảo bên ngoài nào** trong mã nguồn, comment, đường dẫn cấu hình, prompt hay tài liệu.

---

## Phần 1 — 8 Luật Thép, Giải pháp 5 Bẫy Pure Rust & 32 Phân hệ Hệ thống

### 8 Luật Thép (Bắt buộc tuyệt đối cho mọi Implementer AI)

1. **Dependency Firewall tuyệt đối (13 Crates):**
   - `harness-cli` và TUI (`ratatui`) **KHÔNG BAO GIỜ** phụ thuộc trực tiếp vào `harness-core` (chỉ giao tiếp qua `harness-protocol` HTTP/UDS + SSE, trừ chế độ embedded server ở binary entrypoint).
   - Các crate lá nặng (`harness-treesitter`, `harness-codemode`, `harness-lsp`, `harness-index`, `harness-shell`, `harness-edit`) chỉ giao tiếp với `harness-core` qua `trait` nhỏ đặt trong `harness-schema`.
2. **Dynamic Dispatch (`Arc<dyn Trait>`) & Zero Hardcoded Model Versions:**
   - Dùng `Arc<dyn WireProtocol>`, `Arc<dyn EditStrategy>`, `Arc<dyn Tool>`, `Arc<dyn CodeOutliner>`, `Arc<dyn ScriptHost>`, `Arc<dyn SandboxRunner>`, `Arc<dyn PolicyGuard>` thay vì generic lồng nhau để giữ incremental compile 1–3 giây.
   - **Cấm tuyệt đối hardcode số phiên bản model của provider (Claude/Gemini) hoặc nhánh `if version >= ...` hay beta date tag trong mã Rust!** Toàn bộ model ID mạnh nhất mới nhất được giải phân giải động qua `catalog.rs` (`selector = "latest_flagship"` / `"latest_fast"` + biến môi trường ghi đè + `routes.toml`); riêng Qwen giữ 2 profile tường minh `qwen-3.8-27b.toml` và `qwen-4-upcoming.toml`.
3. **Byte-Exact Prefix Determinism (Khóa cứng Prompt Cache):**
   - Mọi JSON payload gửi lên LLM (đặc biệt `tools[]` và schema properties) phải dùng `BTreeMap<String, Value>` (hoặc sort key xác định) và sort `tools` theo `name` tăng dần. Cấm đưa giờ/phút/giây hoặc `git status` vào `system[]`.
4. **Error Hygiene & Cấm nuốt `UserDeclined`:**
   - Cấm `.unwrap()` / `.expect()` ở runtime path (chỉ cho phép trong test). Dùng `thiserror` typed errors theo từng tầng.
   - `ToolOutcome::UserDeclined` là variant tách biệt hoàn toàn với `ToolOutcome::RecoverableError(ToolFailure)` — tuyệt đối không chuyển thành kết quả lỗi gửi lại model.
5. **Single SQLite WAL Writer Thread:**
   - Mọi thao tác ghi SQLite đi qua **đúng 1 writer thread chuyên dụng** dùng `TransactionBehavior::Immediate` (`BEGIN IMMEDIATE`). Event + Projector + Commit Hook (claim / outbox) chạy trong **cùng 1 transaction**; chỉ broadcast SSE sau khi `commit()` thành công.
6. **Zero Built-in Content Censorship (Guardrail tách riêng, mặc định No-op):**
   - Cấm chèn bất kỳ đoạn prompt kiểm duyệt chủ đề/bảo mật/đạo đức thương mại hay preset SDK bên ngoài nào vào `prompts/*.txt` hoặc `harness-core`. Mọi kiểm duyệt tùy biến sau này đi qua `trait PolicyGuard` (mặc định `NoopPolicyGuard` — trả `Pass` 100%).
   - Tầng `harness-ai` chỉ xử lý trung lập tín hiệu từ máy chủ hãng (Anthropic HTTP 200 `stop_reason: "refusal"` + `stop_details`; Gemini `safety_settings` cấu hình mở qua `ModelProfile` TOML).
   - Bảo vệ vận hành máy local (`Landlock` OS Sandbox + Permission rules) hoàn toàn tách biệt và bật/tắt tùy ý (`--auto`, `SandboxProfile::Disabled`).
7. **Clean-Room & Original Brand Identity (CẤM NHẮC NGUỒN THAM KHẢO):**
   - **TUYỆT ĐỐI KHÔNG** nhắc tên, trích dẫn, tạo comment/docstring tham chiếu, hay copy tiền tố thư mục của bất kỳ dự án/agent/harness bên ngoài nào trong mã nguồn (`crates/`, `profiles/`, `prompts/`), thông báo lỗi, bài test, tài liệu README hay git commit message.
   - Mọi thư mục cấu hình/skill/plugin/dữ liệu chỉ dùng namespace độc lập **`.harness/`** (toàn cục `~/.harness/` và cục bộ `<workspace>/.harness/`) và tệp hướng dẫn dự án chuẩn **`AGENTS.md`**.
   - Dự án là một sản phẩm Pure Rust hoàn toàn độc lập mang tên **`rust-harness`** (binary `harness`).
8. **Test-First & Zero Warnings theo từng Task:**
   - Mỗi Task `T01`–`T36` chỉ hoàn thành khi `cargo nextest run -p <crate>` (test offline bằng cassette/mock, không cần mạng hay GPU) xanh 100% và `cargo clippy --workspace --all-targets -- -D warnings` sạch cảnh báo.

### Giải pháp Dứt điểm cho 5 Bẫy Kỹ thuật Pure Rust

| Bẫy kỹ thuật | Nguyên nhân gây hỏng / lỗ | Giải pháp bắt buộc trong `rust-harness` |
|---|---|---|
| **Bẫy 1 — Phá Prompt Cache vì prune sai thời điểm** | Sửa/xóa tool output cũ khi cache Cloud/VRAM đang `Warm` khiến 60K+ token sau đó bị cache miss (lỗ tiền gấp 2–3× hoặc TTFT tăng 10s). | Tách cứng **Write-Time Compression** (luôn bật, 0% phá cache: `Hashline`, `Outline`, `RepoMap`, `Delta Re-Read`, CLI Condensers) khỏi **Retroactive Pruning** (Observation Masking & `Superseded` gom vào `PruneEpoch`, chỉ mở `epoch + 1` khi cache `Cold`, chạm `soft_shake`, vượt điểm hòa vốn, hoặc Compaction). |
| **Bẫy 2 — Viết quá nhiều provider adapter & hardcode đời model** | Viết cứng hàng chục adapter hoặc hardcode số đời model làm mã cũ đi mỗi khi hãng ra flagship mới. | Tách **Protocol × Route × Dynamic Catalog**: chỉ viết chắc **3 Wire Protocols** (`OpenAIChatCompat` cho Qwen 3.8 & Qwen 4 sắp ra mắt, `AnthropicMessages` cho Claude flagship mới nhất, `GeminiGenerateContent` cho Gemini flagship mới nhất) + `ModelResolver` giải phân giải `latest_flagship` động + `ExternalStdioProtocol`. |
| **Bẫy 3 — Lặp logic ghi file giữa các chế độ edit** | Copy-paste khóa file, giữ dòng, permission, formatter, LSP cho từng cách sửa → sinh bug lệch hành vi. | **1 `FileWriteCoordinator` duy nhất** ôm mọi thao tác chạm đĩa (`KeyedMutex`, BOM, `imara-diff`, diff, permission, formatter, `oxc`/`ruff` + LSP); 4 `EditStrategy` (`Hashline`, `StrReplace`, `replace_symbol`, `ast_edit`) là hàm thuần tính bản dự thảo trong RAM; tự chọn chế độ mặc định theo `ModelProfile` TOML. |
| **Bẫy 4 — Mất hệ sinh thái Plugin động khi bỏ Node/Bun** | Binary Rust tĩnh khó mở rộng hook người dùng hoặc gánh 100+ MCP tool. | **Plugin 3 Tầng Pure Rust:** (1) Markdown/TOML hot-reload (`notify`); (2) Nhúng **`rquickjs`** (~1MB, `<1ms`) + **`oxc_transformer`** (strip kiểu `.ts` `<1ms`) chạy cả Code Mode `execute` lẫn user hook plugins `*.js`/`*.ts` mà 0 cần Node/Bun; (3) MCP client. |
| **Bẫy 5 — Sa lầy thời gian biên dịch Rust** | Nhét `tree-sitter`, `rquickjs`, `tantivy`, `ratatui` vào chung crate & dùng generic lồng nhiều tầng làm incremental build mất 30–60s. | **13-Crate Dependency Firewall** + `Arc<dyn Trait>` + feature flags (`lang-core`, `monorepo-index`, `snapcompact`) + linker **`mold`** trong `.cargo/config.toml` → incremental build chỉ **1–3 giây**! |

### Bản đồ 32 Phân hệ Kỹ thuật Pure Rust (`rust-harness`)

| # | Nhóm | Phân hệ | Crate & Module đích | Cơ chế & Bất biến bắt buộc |
|---|---|---|---|---|
| 01 | Loop | Hàng đợi `Inbox` (`steer`/`queue`) | `harness-core/src/inbox.rs` | Idempotency theo `client_msg_id`; `promote()` dưới `KeyedMutex(session_id)` trong cùng SQLite tx xóa inbox + chèn message. |
| 02 | Loop | `SessionRunCoordinator` (Doorbell) | `harness-core/src/coordinator.rs` | `DashMap<SessionId, Arc<Mutex<ExecState>>>` + `pending_wake = widest(prev, scope)` + `CancellationToken`. Khép kín race idle. |
| 03 | Loop | Boot Claim & Crash Recovery | `harness-core/src/recovery.rs` | Write-ahead claim trong tx; boot quét claim cũ, max 10 lần resume (`DEFAULT_MAX_ATTEMPTS = 10`), dọn tool mồ côi → `interrupted`. |
| 04 | Loop | Runner (`drain`/`step`/`attempt`) | `harness-core/src/runner/{drain,step,attempt}.rs` | `JoinSet` chạy tool song song trong lúc stream; dừng theo tool local đã gọi/settled; 3-strike `finish=length`; `max_steps` giữ `tools[]` + `tool_choice="none"`. |
| 05 | Loop | Doom-Loop Detector | `harness-core/src/runner/doom_loop.rs` | Ring-buffer 12 phần tử `(tool_name, blake3(canonical_args))`; trùng 3 lần → chèn synthetic warning ép đổi chiến lược. |
| 06 | Loop | Error Classifier & 3 Retry Budgets | `harness-ai/src/error_classifier.rs`, `harness-core/src/runner/retry.rs` | 10 tầng phân loại lỗi (~40 regex overflow + loại trừ throttle); 3 ngân sách riêng (chung 10 lần 2→10s, mạng 1h trần 30s, timeout ≤3 lần); tôn trọng `x-should-retry` & `Retry-After` (cap 15m). |
| 07 | Store | Event Store WAL & Projector | `harness-core/src/store/{bus,writer,projector}.rs` | `TextDelta`/`ReasoningDelta`/`ToolProgress` ephemeral (lô 100ms, không lưu DB); sự kiện bền ghi `event` + project trong 1 tx SQLite WAL. |
| 08 | LLM | 3 Wire Protocols & `SchemaProjector` | `harness-ai/src/protocols/{openai_chat,anthropic,gemini}.rs` | 3 `WireProtocol` + `RouteDescriptor` TOML + `ExternalStdioProtocol`; `SchemaProjector` phẳng đạt chuẩn Gemini; Anthropic `defer_loading`, server-side `clear_tool_uses` (type đọc từ `routes.toml`), HTTP 200 `stop_reason: "refusal"`; Gemini `safety_settings`. |
| 09 | LLM | Qwen Reasoning & XML/JSON Guard | `harness-ai/src/protocols/qwen_guard.rs` | `thinking-only-guard` cứu `<tool_call>`/`<function=...>` kẹt trong `<think>`; `preserve_thinking`; `enable_thinking`; JSON fence repair + XML fallback. |
| 10 | LLM | Cross-Model Handoff | `harness-core/src/context/to_llm.rs` | Chuyển Qwen/Claude → Gemini gắn `skip_thought_signature_validator`; chỉ replay `signature`/`thoughtSignature` khi `same_model`; cắt assistant lỗi sau tool local cuối đã chạy. |
| 11 | Cache | `CachePolicy`, `CacheMeter` & `PruneEpoch` | `harness-ai/src/cache_policy.rs`, `harness-token/src/cache_meter.rs` | 3 mode cache (`Inline4` Claude, `QwenSystemTail`/`ImplicitPrefix` Qwen, `Implicit` Gemini); `PruneEpoch` cấm mutate prefix khi cache `Warm`. |
| 12 | Token | Dynamic Model Catalog, `Usage` & Anchor Estimator | `harness-ai/src/catalog.rs`, `harness-token/src/estimator.rs` | `ModelResolver` giải phân giải `latest_flagship`/`latest_fast` qua discovery `/models` + ETag refresh + embedded fallback (0 hardcode số đời Claude/Gemini); `Usage` không bao giờ trừ âm; Anchor estimation (`measured + estimated * 1.15`, `tiktoken-rs`). |
| 13 | Edit | `FileWriteCoordinator` × 4 Chiến lược | `harness-edit/src/{coordinator,hashline,str_replace,symbol_edit,ast_edit,endings}.rs` | `KeyedMutex`, BOM, **`imara-diff` Histogram Diff**, permission, formatter, fast lint/LSP dùng chung × `Hashline` (FNV-1a bottom-up), `StrReplace` (3-tier + Hough `missHint`), `replace_symbol` (AST range), `ast_edit` (`ast-grep-core`). |
| 14 | Read | AST Outline, `RepoMap` & Delta Re-Read | `harness-treesitter/src/{outline,repomap,syntax}.rs`, `harness-core/src/tools/{read,repo_map}.rs` | File ≥ 120–220 dòng trả AST Outline; `RepoMap` (`tree-sitter` + Personalized PageRank nén kiến trúc vào 1–2K token); **Delta Re-Read (`read_diff`)** chỉ trả unified diff/`[Unchanged]` khi đọc lại file (0% phá cache!). |
| 15 | Search | `ripgrep` Crates & `nucleo-matcher` | `harness-core/src/tools/{grep,glob}.rs` | Nhúng trực tiếp `ignore` + `grep-searcher` + `grep-regex` + `globset` + `memchr`; cửa sổ 160 ký tự quanh match; **`nucleo-matcher`** fuzzy gợi ý *"Did you mean?"* khi sai đường dẫn. |
| 16 | LSP | Lazy LSP Diagnostics & Symbol Tools | `harness-lsp/src/{manager,client,diagnostics,symbols}.rs` | Lazy spawn LSP (`rust-analyzer`/`tsserver`/`pyright`/`gopls`); debounce 150ms / cap 1500ms → tối đa 15 `ERROR`; cung cấp **`symbol_read`** & **`find_callers`**; fallback `tree-sitter` syntax `<1ms`. |
| 17 | Lint | Ultra-Fast Native Lint (`oxc` + `ruff`) | `harness-lsp/src/fast_lint.rs` | Chạy `oxc_linter` (`<5ms` JS/TS) và `ruff` (`<10ms` Python) ngay sau `edit` để trả lỗi tức thì trước cả khi LSP kịp tính xong. |
| 18 | Index | `tantivy` BM25 Monorepo Indexer | `harness-index/src/lib.rs` | Crate lá sau feature `monorepo-index`: tự xây & cập nhật gia tăng chỉ mục BM25 khi workspace `> 10.000` file, truy vấn top-10 trong `<15ms`. |
| 19 | Token | Semantic CLI/JSON Condensers & `zstd` | `harness-token/src/{condenser,truncation}.rs` | Write-time: mô phỏng `\r`, strip ANSI, gom dòng lặp `[repeated ×N]`, lọc `cargo`/`tsc`/`eslint`/`pytest`/`jest`/`git diff`/`git log`/`git status`/`ls`/`docker`/`kubectl`, rút gọn mảng JSON lặp, grapheme-safe bookend, spill file 7 ngày nén `zstd`. |
| 20 | Token | Observation Masking & `Superseded` Dedup | `harness-token/src/prune.rs` | Giữ nguyên khung `Reasoning + ToolCall`, che `ToolResult` cũ ngoài 8–10 lượt / 25K–40K đuôi tại `PruneEpoch` (cắt ~50% chi phí, 0 LLM cost). |
| 21 | Token | Compaction 5 Tầng & Precompute | `harness-token/src/{shake,reducer,snapcompact}.rs`, `harness-core/src/compaction/` | Write-Time → `shake` (0 LLM) → Native Anthropic → Opt-in Vision `snapcompact` → Structured Summary 7 mục + `<recent-context>` + `<subagents>` + Background Precompute (`ceiling - 60K`) + `SHRINK_STEPS` 5 mức. |
| 22 | Context | 2 Khối System, `InstructionState` & Rules | `harness-core/src/instructions/{state,discovery,agent,rules}.rs` | `blake3` hash + `imara-diff` unified diff qua `<system-update>`; `advanceEpoch` sau compaction; lazy nested `AGENTS.md` + Tool-Boundary Conditional Rules (`globs:`); `instructions_append`; 3 per-model prompts (`qwen-3.8`, `claude`, `gemini`). |
| 23 | Safety | Permission Lattice & Saved Approvals | `harness-core/src/permission/{policy,service,saved}.rs` | Last-match-wins, `deny > ask > allow`, short-circuit deny, `ignore_case` macOS/Win, `Always` batch auto-resolve, `Reject` hủy cùng message, cấm save `.env`→`*` & `sudo *`/`bash -c *`. |
| 24 | Safety | OS Kernel Sandbox & Background Shell | `harness-shell/src/{runner,pty,splitter,sandbox_linux,sandbox_macos}.rs` | Timeout 2m chuyển nền (`<background-result>`), `SIGTERM → 3s → SIGKILL`, `portable-pty`; **Linux `Landlock` ABI V3** (`pre_exec`) + `bwrap` tùy chọn + macOS Seatbelt (`ReadOnlyWorkspace` / `WorkspaceWrite` / `Disabled`). |
| 25 | Safety | Pluggable `PolicyGuard` Slot (No-op) | `harness-schema/src/guard.rs` | `trait PolicyGuard` mặc định `NoopPolicyGuard` (0 kiểm duyệt chủ đề dựng sẵn); sẵn sàng để người dùng tự cài đặt luật riêng sau này. |
| 26 | UX | Tri-Model Role Routing, `/btw` & `/tree` | `harness-core/src/{routing,sidechat,tree}.rs` | 5 vai (`primary`, `deep`, `compact`, `vision`, `smol`); `/btw` read-only cache-hit side tangent; `/tree` nhánh DAG trên `session_message.parent_id`. |
| 27 | Agent | Subagents nền & Transactional Outbox | `harness-core/src/subagent/{tool,outbox}.rs` | Depth=1 mặc định; `max_concurrent_subagents` theo `ModelProfile` (2 trên 1 GPU local); ghi `subagent_outbox` trong cùng tx terminal rồi giao `steer` + wake cha. |
| 28 | Ext | Skills Discovery & MCP Client | `harness-core/src/{skill,mcp}/` | Quét `.harness/skills/` (tối đa 6 tầng, watcher 100ms) + delta `<available_skills>`; MCP stdio + StreamableHTTP + OAuth PKCE; định tuyến MCP tools vào Code Mode. |
| 29 | Ext | `rquickjs` Code Mode & `.js`/`.ts` Plugins | `harness-codemode/src/{runtime,catalog,plugins,ts_strip}.rs` | Nhúng `rquickjs` (~1MB, RAM 64MB, timeout, `max_tool_calls=50`, permission nested) chạy Code Mode `execute` + user hooks `~/.harness/plugins/*.{js,ts}` qua **`oxc_transformer`** strip `.ts` `<1ms` (0 Node/Bun!). |
| 30 | Git | Pure Rust `gix` Git Engine & Shadow Snapshot | `harness-core/src/{git_native.rs,snapshot.rs,revert.rs}` | Pure Rust `gix` (0 fork, thread-safe, `.gitignore`, blob diff `imara-diff`); Shadow Snapshot tách riêng `<data>/snapshot/<project>/<hash>` không chạm `.git` user; 3-Phase Revert (`stage`/`clear`/`commit`). |
| 31 | App | `axum` HTTP/UDS SSE Server & `ratatui` TUI | `harness-protocol`, `harness-server`, `harness-cli` | REST + SSE daemon tách biệt khỏi core; `ratatui` TUI hiển thị live **Cache Meter**, tỷ lệ nén, chi phí, Permission Modal. |
| 32 | Eval | Headless CLI & A/B Eval Suite (`--repeat 3`) | `harness-core/src/eval.rs`, `harness-cli/src/{run,eval_cmd}.rs` | `harness run --auto` & `harness eval --repeat 3` đọc `step_metrics` so sánh `pass@1`, `cost_per_resolved_task`, `cache_hit_ratio`, `edit_first_try_rate`. |

---

## Phần 2 — Cấu trúc Cargo Workspace (13 Crates), `Cargo.toml` & Fast Build Config

### Sơ đồ Dependency Firewall (Hướng mũi tên = `depends on`)

```text
                       ┌────────────────────┐
                       │   harness-schema   │  (0 heavy deps: serde, blake3, ulid, thiserror)
                       │  DTOs + Core Traits│  (+ trait PolicyGuard = NoopPolicyGuard mặc định)
                       └─────────▲──────────┘
       ┌───────────┬─────────────┼─────────────┬──────────────┬───────────────┐
       │           │             │             │              │               │
┌──────┴─────┐ ┌───┴──────┐ ┌────┴─────┐ ┌─────┴───────┐ ┌────┴───────┐ ┌─────┴────────┐
│ harness-ai │ │harness-  │ │harness-  │ │harness-     │ │harness-lsp │ │harness-shell │
│ (3 protos) │ │token     │ │edit      │ │treesitter   │ │(lsp+oxc)   │ │(landlock/pty)│
└──────▲─────┘ └────▲─────┘ └────▲─────┘ └──────▲──────┘ └─────▲──────┘ └──────▲───────┘
       │            │            │              │              │               │
       │            └────────────┼──────────────┴──────────────┴───────────────┘
       │                         │  (chỉ nối qua `Arc<dyn Trait>` ở harness-schema!)
       │                   ┌─────┴────────────┐      ┌──────────────────────────────┐
       └───────────────────┤   harness-core   │◄─────┤ harness-codemode (rquickjs)  │
                           │ (Loop/SQLite/MCP)│◄─────┤ harness-index    (tantivy*)  │
                           └────────▲─────────┘      └──────────────────────────────┘
                                    │                  (*optional feature `monorepo-index`)
                       ┌────────────┴────────────┐
                       │     harness-server      │  (axum HTTP/UDS + SSE)
                       └────────────▲────────────┘
                                    │
                       ┌────────────┴────────────┐
                       │    harness-protocol     │  (Shared Wire DTOs — KHÔNG phụ thuộc core!)
                       └────────────▲────────────┘
                                    │
                       ┌────────────┴────────────┐
                       │       harness-cli       │  (run headless, serve, tui ratatui, eval)
                       └─────────────────────────┘
```

### Cây thư mục file chi tiết

```text
rust-harness/
├── .cargo/
│   └── config.toml                    # mold fast linker trên Linux
├── Cargo.toml                         # Workspace root + shared dependencies
├── profiles/                          # Built-in ModelProfile & Route TOML (nhúng include_bytes!, 0 hardcode version Claude/Gemini)
│   ├── qwen-3.8-27b.toml              # Qwen 3.8 27B
│   ├── qwen-4-upcoming.toml           # Qwen 4 (sắp ra mắt)
│   ├── claude-flagship.toml           # Claude mạnh nhất mới nhất (selector = "latest_flagship")
│   ├── gemini-flagship.toml           # Gemini mạnh nhất mới nhất (selector = "latest_flagship")
│   └── routes.toml                    # Default route definitions + protocol capability tags (SGLang, vLLM, DashScope, Anthropic, Gemini)
├── prompts/
│   ├── base.txt                       # ~280 tokens (protocol rules + ${TOOL_GUIDANCE}, 0 kiểm duyệt chủ đề)
│   ├── qwen-3.8.txt                   # ~200 words append (Qwen 3.8 27B)
│   ├── qwen-4.txt                     # ~150 words append (Qwen 4 sắp ra mắt)
│   ├── claude.txt                     # ~50 words append (Claude Flagship)
│   ├── gemini.txt                     # ~80 words append (Gemini Flagship)
│   └── compaction.txt                 # 7-section structured summary + update/merge + NUDGE
└── crates/
    ├── harness-schema/src/
    │   ├── lib.rs
    │   ├── message.rs                 # Canonical Message, ContentPart, Role, Usage
    │   ├── event.rs                   # LLMEvent (stream), SessionEvent (durable vs ephemeral)
    │   ├── error.rs                   # AiError, ToolFailure, ToolOutcome
    │   ├── tool.rs                    # ToolDef, ToolResult, ToolContext, Tool trait
    │   ├── permission.rs              # PermissionRule, PermissionPolicy, PermissionRequest/Reply
    │   ├── agent.rs                   # AgentInfo, AgentMode, ModelProfile, RouteDescriptor
    │   ├── guard.rs                   # GuardDecision, trait PolicyGuard, NoopPolicyGuard
    │   └── ports.rs                   # Firewall traits: WireProtocol, EditStrategy, CodeOutliner, LspClient, ScriptHost, SandboxRunner
    ├── harness-ai/src/
    │   ├── lib.rs
    │   ├── client.rs                  # prepare_request() pipeline (sanitize -> sort -> project -> cache)
    │   ├── schema_projector.rs        # Gemini-safe JSON schema flattener ($ref, allOf, $defs, additionalProperties)
    │   ├── cache_policy.rs            # Inline4 (Claude), QwenSystemTail, ImplicitPrefix, Implicit (Gemini)
    │   ├── error_classifier.rs        # 10-priority classifier + ~40 overflow regexes + throttle exclusions
    │   ├── catalog.rs                 # ModelResolver (`latest_flagship`/`latest_fast`) + `/models` discovery + ETag refresh + embedded fallback
    │   └── protocols/
    │       ├── mod.rs                 # Protocol registry + ExternalStdioProtocol escape hatch
    │       ├── openai_chat.rs         # Qwen 3.8 27B & Qwen 4 sắp ra mắt (+ SGLang/vLLM/DashScope/OpenRouter)
    │       ├── qwen_guard.rs          # thinking-only-guard + XML <function=...> parser + JSON fence repair
    │       ├── anthropic.rs           # Claude flagship Messages + 4 breakpoints + thinking + defer_loading + clear_tool_uses + HTTP 200 refusal
    │       └── gemini.rs              # Gemini flagship streamGenerateContent + thoughtSignature + skip_validator + fnResp split + safety_settings
    ├── harness-token/src/
    │   ├── lib.rs
    │   ├── estimator.rs               # tiktoken-rs + Anchor estimation (measured + estimated * 1.15)
    │   ├── cache_meter.rs             # TTL tracker + break-even formula + PruneEpoch trigger gate
    │   ├── condenser.rs               # Write-time Semantic CLI & JSON condensers (\r, ANSI, cargo, tsc, pytest, git, docker, JSON collapse)
    │   ├── truncation.rs              # Grapheme-safe head/tail truncation + zstd spill files (7-day retention)
    │   ├── prune.rs                   # Superseded read dedup + Observation Masking (giữ 8-10 lượt / 25K-40K đuôi)
    │   ├── shake.rs                   # Tier-1 zero-LLM compaction (spill bulky blocks -> local://spill/)
    │   ├── reducer.rs                 # 5-level flatten_and_drop_oldest + SHRINK_STEPS [0.85, 0.70, 0.50, 0.35]
    │   └── snapcompact.rs             # Optional feature `snapcompact`: tiny-skia + png rasterization
    ├── harness-edit/src/
    │   ├── lib.rs
    │   ├── coordinator.rs             # FileWriteCoordinator: KeyedMutex, BOM, imara-diff, permission, formatter, fast lint/LSP
    │   ├── endings.rs                 # BOM preserve + Histogram line-ending preservation (`imara-diff`)
    │   ├── hashline.rs                # FNV-1a 2-char anchor, atomic bottom-up apply, prefix-strip, ±4 self-heal
    │   ├── str_replace.rs             # 3-tier LF-view matcher + Hough-voting missHint (≤12 lines)
    │   ├── symbol_edit.rs             # replace_symbol theo AST range
    │   └── ast_edit.rs                # ast-grep-core structural refactor đa file
    ├── harness-treesitter/src/
    │   ├── lib.rs
    │   ├── outline.rs                 # Structural AST outline for files >= threshold (feature-gated grammars)
    │   ├── repomap.rs                 # Structural RepoMap: symbol dependency graph + Personalized PageRank (1K-2K tokens)
    │   └── syntax.rs                  # <1ms syntax error fallback when LSP unavailable
    ├── harness-lsp/src/
    │   ├── lib.rs
    │   ├── manager.rs                 # Lazy per-language LSP server lifecycle
    │   ├── client.rs                  # Async JSON-RPC stdio client (`lsp-types`)
    │   ├── diagnostics.rs             # 150ms debounce / 1500ms cap, max 15 ERRORs formatter
    │   ├── symbols.rs                 # symbol_read & find_callers qua LSP
    │   └── fast_lint.rs               # Ultra-fast post-edit lint: `oxc_linter` (<5ms JS/TS) + `ruff` (<10ms Py)
    ├── harness-index/src/             # Optional crate (`monorepo-index`): `tantivy` BM25 monorepo indexer (>10K-50K files)
    │   └── lib.rs
    ├── harness-shell/src/
    │   ├── lib.rs
    │   ├── runner.rs                  # Timeout-to-background job, SIGTERM->3s->SIGKILL, output capture
    │   ├── pty.rs                     # Persistent pipe/PTY runner (`portable-pty`)
    │   ├── splitter.rs                # Lightweight subcommand & ARITY extractor (~250 LOC for UX ask rules)
    │   ├── sandbox_linux.rs           # Linux Landlock ABI V3 (`pre_exec`) + optional `bwrap`
    │   └── sandbox_macos.rs           # macOS `sandbox-exec` SBPL profile generator
    ├── harness-codemode/src/
    │   ├── lib.rs
    │   ├── runtime.rs                 # Embedded `rquickjs` sandbox: mem limit, timeout, maxToolCalls
    │   ├── catalog.rs                 # Budgeted 2K-token inline catalog + `search()` + delta update 1K
    │   ├── plugins.rs                 # User hook plugins `~/.harness/plugins/*.{js,ts}` + hot-reload
    │   └── ts_strip.rs                # `oxc_transformer` strip TypeScript types <1ms cho plugin `*.ts`
    ├── harness-core/src/
    │   ├── lib.rs
    │   ├── store/
    │   │   ├── mod.rs
    │   │   ├── migrations/0001_init.sql
    │   │   ├── writer.rs              # Single dedicated SQLite WAL writer thread (`BEGIN IMMEDIATE`)
    │   │   ├── bus.rs                 # Durable publish (tx + projector + commit hook) + broadcast SSE
    │   │   └── projector.rs           # Deterministic projections for all SessionEvents
    │   ├── inbox.rs                   # Idempotent admission + `steer`/`queue` promoteUnlocked
    │   ├── coordinator.rs             # SessionRunCoordinator (Doorbell pattern + interrupt)
    │   ├── recovery.rs                # Boot claim scanner + max 10 resume + stale tool settlement
    │   ├── runner/
    │   │   ├── drain.rs               # Outer turn loop + advanceToStep + idle hook + title gen
    │   │   ├── step.rs                # Step policy: auto-compact, max_steps, 3-strike length, retry
    │   │   ├── attempt.rs             # Stream + parallel JoinSet tools + 16-row decision table
    │   │   ├── retry.rs               # 3 independent retry budgets (general / network / timeout)
    │   │   └── doom_loop.rs           # blake3(tool, args) ring-buffer detector
    │   ├── context/
    │   │   ├── to_llm.rs              # DB history -> canonical LLMRequest + Cross-Model Handoff rules
    │   │   └── request_builder.rs     # System[0..1], outputLimit, boundImages (25/15MiB hysteresis), affinity
    │   ├── instructions/
    │   │   ├── state.rs               # Content-addressed blake3 + imara-diff delta + advanceEpoch + rewind
    │   │   ├── discovery.rs           # AGENTS.md walk-up + watcher (100ms debounce)
    │   │   ├── agent.rs               # {selected, unselected} Plan/Build switching
    │   │   └── rules.rs               # Lazy nested AGENTS.md + Tool-Boundary Conditional Rules (`globs:`)
    │   ├── permission/
    │   │   ├── policy.rs              # Lattice evaluator (Rules/Override/Restrict/Fallback) + reachableEffects
    │   │   ├── service.rs             # Pending Deferred (`oneshot`), Always batch auto-resolve, Reject batch
    │   │   └── saved.rs               # Per-project saved approvals (forbid `.env` -> `*`, `sudo *`)
    │   ├── compaction/
    │   │   ├── mod.rs                 # 5-tier orchestrator (Write-Time -> shake -> Native -> snap -> Summary)
    │   │   ├── precompute.rs          # Background precompute at `ceiling - 60K` when cache warm
    │   │   └── summary.rs             # 7-section template, NUDGE 1x, <subagents>/<background-work> injection
    │   ├── subagent/
    │   │   ├── tool.rs                # Depth=1, max_concurrent_subagents, background start
    │   │   └── outbox.rs              # Transactional outbox -> parent inbox `steer` + wake
    │   ├── tools/                     # Built-in: read (+read_diff), edit, write, shell, grep, glob, repo_map, symbol_read, find_callers, replace_symbol, ast_edit, webfetch, skill, subagent, execute
    │   ├── skill/                     # Discovery (.harness/skills/) + delta `<available_skills>`
    │   ├── mcp/                       # stdio + StreamableHTTP + OAuth PKCE + Code Mode routing
    │   ├── routing.rs                 # Intent-based Tri-Model role routing (primary/deep/compact/vision/smol)
    │   ├── sidechat.rs                # `/btw` cache-hitting read-only side tangent
    │   ├── tree.rs                    # `/tree` branching on `session_message.parent_id`
    │   ├── git_native.rs              # Pure Rust `gix` (gitoxide) status/diff/snapshot engine (0 fork)
    │   ├── snapshot.rs                # Shadow git snapshot repo + 3-phase revert (stage/clear/commit)
    │   └── eval.rs                    # `--repeat 3` benchmark runner + `step_metrics` A/B comparator
    ├── harness-protocol/src/          # Pure HTTP/UDS REST + SSE wire DTOs (0 dep on core)
    ├── harness-server/src/            # `axum` HTTP/UDS server + SSE event stream + permission reply API
    └── harness-cli/src/               # Binary `harness`: `run` (headless), `serve`, `tui` (`ratatui`), `eval`
```

### Root `Cargo.toml` & `.cargo/config.toml` chuẩn

```toml
# Cargo.toml (Workspace Root)
[workspace]
resolver = "2"
members = [
  "crates/harness-schema",
  "crates/harness-ai",
  "crates/harness-token",
  "crates/harness-edit",
  "crates/harness-treesitter",
  "crates/harness-lsp",
  "crates/harness-index",
  "crates/harness-shell",
  "crates/harness-codemode",
  "crates/harness-core",
  "crates/harness-protocol",
  "crates/harness-server",
  "crates/harness-cli",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.85"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
tokio-util = { version = "0.7", features = ["rt"] }
futures = "0.3"
async-trait = "0.1"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream", "gzip"] }
eventsource-stream = "0.2"
rusqlite = { version = "0.32", features = ["bundled", "modern_sqlite"] }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }
toml = "0.8"
schemars = "0.8"
jsonschema = "0.26"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
blake3 = "1.5"
ulid = { version = "1", features = ["serde"] }
dashmap = "6"
imara-diff = "0.2"
nucleo-matcher = "0.3"
unicode-segmentation = "1.12"
regex = "1"
aho-corasick = "1"
memchr = "2"
zstd = "0.13"
rayon = "1.10"
strip-ansi-escapes = "0.2"
tiktoken-rs = "0.6"
ignore = "0.4"
grep-searcher = "0.1"
grep-regex = "0.1"
globset = "0.4"
notify = "7"
tree-sitter = "0.24"
lsp-types = "0.97"
portable-pty = "0.8"
landlock = "0.4"
nix = { version = "0.29", features = ["signal", "process"] }
rquickjs = { version = "0.9", features = ["futures", "macro", "parallel"] }
axum = { version = "0.8", features = ["macros"] }
ratatui = "0.29"
crossterm = "0.28"
clap = { version = "4", features = ["derive"] }
ast-grep-core = "0.35"
gix = { version = "0.70", default-features = false, features = ["max-performance-safe", "status", "revision", "blob-diff"] }
tantivy = "0.22"
oxc_parser = "0.56"
oxc_allocator = "0.56"
oxc_transformer = "0.56"
# Optional feature `snapcompact`: tiny-skia = "0.11", png = "0.17"
```

```toml
# .cargo/config.toml (Fast incremental link ~0.4-0.8s trên Linux)
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

---

## Phần 3 — Hợp đồng Kiểu dữ liệu Cốt lõi (`harness-schema`) & SQLite DDL

### A. Core Rust Contracts (`crates/harness-schema/src/`)

```rust
// --- 1. Canonical Usage (KHÔNG BAO GIỜ trừ ra số âm) ---
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,              // Tổng bao gồm = non_cached + cache_read + cache_write
    pub non_cached_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,             // Tổng bao gồm = visible + reasoning
    pub reasoning_tokens: u64,
    pub provider_reported_cost_usd: Option<f64>,
}
impl Usage {
    pub fn visible_output_tokens(&self) -> u64 {
        self.output_tokens.saturating_sub(self.reasoning_tokens)
    }
    pub fn cache_hit_ratio(&self) -> f64 {
        if self.input_tokens == 0 { 0.0 }
        else { self.cache_read_input_tokens as f64 / self.input_tokens as f64 }
    }
}

// --- 2. Canonical Content Parts & Messages ---
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String, provider_meta: Option<BTreeMap<String, Value>> },
    Reasoning {
        text: String,
        signature: Option<String>,       // Anthropic signature / Gemini thoughtSignature
        redacted_data: Option<String>,   // Anthropic redacted_thinking
        provider_meta: Option<BTreeMap<String, Value>>,
    },
    Media { mime_type: String, base64_data: String, source_path: Option<String> },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        provider_executed: bool,
        signature: Option<String>,       // Gemini thoughtSignature trên functionCall
    },
    ToolResult {
        call_id: String,
        name: String,
        content: Vec<ToolResultBlock>,
        is_error: bool,
    },
    Compaction { encrypted: Option<String>, summary_xml: Option<String> },
    EffortMarker { effort: String, previous: Option<String> },
}

// --- 3. Normalized Stream Events ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMEvent {
    StepStart,
    TextStart, TextDelta(String), TextEnd,
    ReasoningStart, ReasoningDelta(String), ReasoningEnd { signature: Option<String>, redacted: Option<String> },
    ToolInputStart { id: String, name: String },
    ToolInputDelta { id: String, delta: String },
    ToolInputError { id: String, name: String, raw_excerpt: String, reason: ToolInputErrorKind },
    ToolCall { id: String, name: String, arguments: Value, provider_executed: bool, signature: Option<String> },
    ToolResultHosted { call_id: String, content: Vec<ToolResultBlock>, is_error: bool },
    Finish { reason: FinishReason, usage: Usage },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolInputErrorKind { MalformedJson, TruncatedByLength, IncompleteStream }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason { Stop, ToolCalls, Length, ContentFilter, Unknown }

// --- 4. Typed Errors & ToolOutcome (KHÔNG BAO GIỜ nuốt UserDeclined!) ---
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
pub enum AiError {
    #[error("context overflow: {message}")]
    ContextOverflow { message: String, raw_body: String },
    #[error("payload too large: {message}")]
    PayloadTooLarge { message: String },
    #[error("rate limited (retry_after_ms={retry_after_ms:?}): {message}")]
    RateLimit { message: String, retry_after_ms: Option<u64>, should_retry_header: Option<bool> },
    #[error("quota exceeded: {message}")]
    QuotaExceeded { message: String },
    #[error("authentication failed: {message}")]
    Authentication { message: String },
    #[error("content policy violation: {message}")]
    ContentPolicy { message: String },
    #[error("provider internal error ({status:?}): {message}")]
    ProviderInternal { status: Option<u16>, message: String, retry_after_ms: Option<u64>, should_retry_header: Option<bool> },
    #[error("transport error ({operation:?}, network_down={network_down}): {message}")]
    Transport { operation: TransportOp, accepted: bool, network_down: bool, message: String },
    #[error("incomplete stream: {message}")]
    IncompleteStream { message: String },
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("timeout: {message}")]
    Timeout { message: String },
    #[error("unknown provider error: {message}")]
    UnknownProvider { message: String, raw_body: String },
}

#[derive(Debug, Clone)]
pub enum ToolOutcome {
    Completed(ToolResult),
    RecoverableError(ToolFailure), // BlockedError, CorrectedError, validation/runtime error -> gửi lại model
    UserDeclined,                  // Reject trần -> DỪNG NGAY CẢ TURN, tuyệt đối không biến thành RecoverableError!
}

// --- 5. Permission Lattice (deny > ask > allow) & Pluggable PolicyGuard (No-op mặc định) ---
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PermissionEffect { Allow = 0, Ask = 1, Deny = 2 } // max() = phép gộp dàn deny > ask > allow!

#[derive(Debug, Clone)]
pub enum GuardDecision { Pass, Transform(String), Block { reason: String } }

/// Điểm cắm Guardrail tùy biến của người dùng (mặc định `NoopPolicyGuard` — 0 kiểm duyệt dựng sẵn).
#[async_trait::async_trait]
pub trait PolicyGuard: Send + Sync {
    async fn inspect_input(&self, _session_id: &str, _text: &str) -> GuardDecision { GuardDecision::Pass }
    async fn inspect_tool_call(&self, _tool: &str, _args: &Value) -> GuardDecision { GuardDecision::Pass }
    async fn inspect_output(&self, _session_id: &str, _text: &str) -> GuardDecision { GuardDecision::Pass }
}
pub struct NoopPolicyGuard;
impl PolicyGuard for NoopPolicyGuard {}

// --- 6. Dynamic ModelProfile Contract (0 hardcode số đời Claude/Gemini!) ---
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoOrU64 { Auto(String), Fixed(u64) } // "auto" đọc từ catalog.rs / provider /models

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSelectorSpec {
    pub provider: String,             // "qwen" | "anthropic" | "google"
    pub selector: String,             // "exact:<id>" | "latest_flagship" | "latest_fast" | "latest_family:<prefix>"
    pub env_override: String,         // vd: "HARNESS_CLAUDE_MODEL" / "HARNESS_GEMINI_MODEL" / "HARNESS_QWEN4_MODEL"
    pub route: String,                // trỏ tới [routes.<name>] trong profiles/routes.toml
    pub protocol: String,             // "openai_chat_compat" | "anthropic_messages" | "gemini_generate_content"
    pub prompt_tune_file: String,
    pub prompt_tune_mode: String,     // "append"
}
```

### B. Canonical `profiles/*.toml` & `profiles/routes.toml` (Chuẩn duy nhất — 0 hardcode phiên bản Claude/Gemini)

> **Quy tắc TOML bắt buộc:** Toàn bộ 4 profile dùng **cùng 1 schema thống nhất** (`[model]`, `[context]`, `[reasoning]`, `[tools]`, `[cache]`, `[limits]`). Claude & Gemini **tuyệt đối không hardcode số đời model hay trần context/output cố định** (dùng `selector = "latest_flagship"` + `"auto"` + tỷ lệ `soft_shake_ratio`). Riêng Qwen giữ 2 profile tường minh cho **Qwen 3.8 27B** và **Qwen 4 (sắp ra mắt)**.

```toml
# profiles/qwen-3.8-27b.toml
[model]
provider = "qwen"
selector = "exact:qwen3.8-27b"
env_override = "HARNESS_QWEN38_MODEL"
route = "qwen_local_sglang"         # Đổi sang "qwen_local_vllm" hoặc "qwen_dashscope" tùy môi trường
protocol = "openai_chat_compat"
prompt_tune_file = "prompts/qwen-3.8.txt"
prompt_tune_mode = "append"

[context]
native_window = 262144
effective_window = 110000           # Vùng ngọt chống Context Rot & tràn VRAM 27B
soft_shake_tokens = 62000           # Chạy shake + Observation Masking (0 LLM cost)
keep_recent_tokens = 16000
max_output_tokens = 16384

[reasoning]
mode = "enabled"
preserve_thinking = true
thinking_entrapment_guard = true
smol_disables_thinking = true

[tools]
edit_mode = "hashline"
diff_engine = "imara_histogram"
ast_edit = true
read_outline_threshold_lines = 120
delta_reread = true
repomap_budget_tokens = 1500
observation_masking_window = 8
fast_lint = ["oxc", "ruff"]
git_engine = "gix"
ts_plugin_strip = "oxc"
tantivy_threshold_files = 10000
max_direct_tools = 8
wire_xml_fallback = true
json_repair = true
policy_guard = "noop"               # Zero built-in topic censorship

[cache]
policy = "implicit_prefix"          # Đổi thành "qwen_system_tail" nếu chạy route qwen_dashscope
ttl_seconds = 300

[sampling]
temperature = 0.65
top_p = 0.95
presence_penalty = 0.1

[limits]
doom_loop_repeat = 3
max_concurrent_subagents = 2
```

```toml
# profiles/qwen-4-upcoming.toml
[model]
provider = "qwen"
selector = "latest_family:qwen4"    # Tự chọn Qwen 4 mạnh nhất ngay khi ra mắt (hoặc $HARNESS_QWEN4_MODEL)
env_override = "HARNESS_QWEN4_MODEL"
route = "qwen_dashscope"            # Hoặc "qwen_local_sglang" khi tự host
protocol = "openai_chat_compat"
prompt_tune_file = "prompts/qwen-4.txt"
prompt_tune_mode = "append"

[context]
native_window = "auto"              # Đọc tự động theo spec chính thức Qwen 4
effective_window = "auto"           # Tính động từ catalog/spec Qwen 4
soft_shake_ratio = 0.60             # Kích hoạt shake tại 60% effective_window
keep_recent_tokens = 20000
max_output_tokens = "auto"

[reasoning]
mode = "enabled"
preserve_thinking = true
thinking_entrapment_guard = true    # Giữ làm lưới an toàn (0 overhead khi sạch)
smol_disables_thinking = true

[tools]
edit_mode = "hashline"              # Chạy Eval A/B hashline vs str_replace khi Qwen 4 ra mắt
diff_engine = "imara_histogram"
ast_edit = true
read_outline_threshold_lines = 200
delta_reread = true
repomap_budget_tokens = 2000
observation_masking_window = 10
fast_lint = ["oxc", "ruff"]
git_engine = "gix"
ts_plugin_strip = "oxc"
tantivy_threshold_files = 10000
max_direct_tools = 10
wire_xml_fallback = false
json_repair = true
policy_guard = "noop"

[cache]
policy = "qwen_system_tail"         # Hoặc "implicit_prefix" nếu chạy SGLang local
ttl_seconds = 300
allow_snapcompact = false

[sampling]
temperature = 0.6
top_p = 0.95
presence_penalty = 0.0

[limits]
doom_loop_repeat = 3
max_concurrent_subagents = 4
```

```toml
# profiles/claude-flagship.toml
[model]
provider = "anthropic"
selector = "latest_flagship"        # ModelResolver tự chọn Claude mạnh nhất mới nhất (0 hardcode đời model!)
env_override = "HARNESS_CLAUDE_MODEL"
route = "anthropic_direct"
protocol = "anthropic_messages"
prompt_tune_file = "prompts/claude.txt"
prompt_tune_mode = "append"

[context]
native_window = "auto"              # Đọc động từ catalog.rs / provider metadata
effective_window = "auto"           # Tự tính ceiling = window - max(15%, 16K)
soft_shake_ratio = 0.65             # Kích hoạt shake tại 65% ceiling (không hardcode số cứng!)
keep_recent_tokens = 18000
max_output_tokens = "auto"          # Đọc động theo giới hạn mới nhất của flagship

[reasoning]
mode = "auto"                       # Đọc capability từ catalog (adaptive + effort hoặc enabled + budget)
effort = "high"
display = "summarized"
prefix_mismatch_behavior = "drop_block"

[tools]
edit_mode = "str_replace"           # Hoặc "hashline" để đồng nhất khi đổi từ Qwen
diff_engine = "imara_histogram"
ast_edit = true
read_outline_threshold_lines = 220
delta_reread = true
repomap_budget_tokens = 2000
observation_masking_window = 10
fast_lint = ["oxc", "ruff"]
git_engine = "gix"
ts_plugin_strip = "oxc"
tantivy_threshold_files = 10000
defer_loading = true                # Anthropic Tool Search cho tool ít dùng
clear_tool_uses_server = true       # Server-side context editing (type tag đọc từ routes.toml)
max_direct_tools = 12
policy_guard = "noop"               # Zero built-in topic censorship

[cache]
policy = "anthropic_inline4"        # tool cuối -> sys[0] -> sys[1] -> tail(1)
ttl_seconds = 300
strict_warm_prune_guard = true

[limits]
doom_loop_repeat = 3
max_concurrent_subagents = 6
```

```toml
# profiles/gemini-flagship.toml
[model]
provider = "google"
selector = "latest_flagship"        # ModelResolver tự chọn Gemini mạnh nhất mới nhất (vai smol dùng "latest_fast")
env_override = "HARNESS_GEMINI_MODEL"
route = "google_ai_studio"
protocol = "gemini_generate_content"
prompt_tune_file = "prompts/gemini.txt"
prompt_tune_mode = "append"

[context]
native_window = "auto"              # Đọc động từ catalog.rs / endpoint /models
effective_window = "auto"           # Tự giới hạn an toàn theo tỷ lệ hoặc trần cấu hình user
soft_shake_ratio = 0.55             # Kích hoạt shake tại 55% effective_window
keep_recent_tokens = 20000
max_output_tokens = "auto"          # Đọc động theo giới hạn mới nhất của flagship

[reasoning]
mode = "auto"
preserve_thought_signature = true
unsigned_tool_sentinel = "skip_thought_signature_validator"

[tools]
edit_mode = "hashline"
diff_engine = "imara_histogram"
ast_edit = true
read_outline_threshold_lines = 220
delta_reread = true
repomap_budget_tokens = 2000
observation_masking_window = 10
fast_lint = ["oxc", "ruff"]
git_engine = "gix"
ts_plugin_strip = "oxc"
tantivy_threshold_files = 10000
strict_schema_sanitizer = true
separate_user_turn_after_fn_resp = true
safety_settings = "BLOCK_NONE"      # Cấu hình mở tránh báo động giả
policy_guard = "noop"

[cache]
policy = "gemini_implicit"          # KHÔNG gửi inline breakpoint đuôi!
ttl_seconds = 300

[limits]
doom_loop_repeat = 3
max_concurrent_subagents = 6
```

```toml
# profiles/routes.toml — Địa chỉ API, Discovery /models, Capability Tags & Role Routing
[roles]
primary = "qwen-3.8-27b"            # Đổi sang "qwen-4-upcoming" ngay khi Qwen 4 phát hành
deep = "claude-flagship"
compact = "gemini-flagship"
vision = "gemini-flagship"
smol = "qwen-3.8-27b"               # Tự tắt enable_thinking=false (hoặc dùng gemini selector="latest_fast")

[routes.qwen_local_sglang]
protocol = "openai_chat_compat"
base_url = "http://127.0.0.1:30000/v1"
models_endpoint = "/models"
api_key_env = "HARNESS_LOCAL_API_KEY"
prefix_cache_engine = "sglang_radix"

[routes.qwen_local_vllm]
protocol = "openai_chat_compat"
base_url = "http://127.0.0.1:8000/v1"
models_endpoint = "/models"
api_key_env = "HARNESS_LOCAL_API_KEY"
prefix_cache_engine = "vllm_apc"

[routes.qwen_dashscope]
protocol = "openai_chat_compat"
base_url = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
models_endpoint = "/models"
api_key_env = "DASHSCOPE_API_KEY"

[routes.anthropic_direct]
protocol = "anthropic_messages"
base_url = "https://api.anthropic.com"
models_endpoint = "/v1/models"
api_key_env = "ANTHROPIC_API_KEY"
# Các beta/type tag server-side đọc động từ route config thay vì hardcode trong Rust:
context_editing_strategy = "auto_from_catalog"

[routes.google_ai_studio]
protocol = "gemini_generate_content"
base_url = "https://generativelanguage.googleapis.com"
models_endpoint = "/v1beta/models"
api_key_env = "GEMINI_API_KEY"
```

### C. SQLite Event Store DDL (`crates/harness-core/src/store/migrations/0001_init.sql`)

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

CREATE TABLE event_sequence (
    aggregate_id TEXT PRIMARY KEY,
    seq          INTEGER NOT NULL
);

CREATE TABLE event (
    id           TEXT PRIMARY KEY,
    aggregate_id TEXT NOT NULL,
    seq          INTEGER NOT NULL,
    event_type   TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    UNIQUE (aggregate_id, seq)
);

CREATE TABLE session (
    id                TEXT PRIMARY KEY,
    parent_id         TEXT,                 -- NULL = session gốc; có giá trị = subagent
    fork_source_id    TEXT,
    title             TEXT NOT NULL,
    title_is_default  INTEGER NOT NULL DEFAULT 1,
    agent_id          TEXT NOT NULL,
    model_profile     TEXT NOT NULL,
    cwd               TEXT NOT NULL,
    workspace_root    TEXT NOT NULL,
    active_branch     TEXT,                 -- message_id hiện tại cho /tree
    claim_owner       TEXT,                 -- Write-ahead claim phát hiện crash khi boot
    resume_attempts   INTEGER NOT NULL DEFAULT 0,
    revert_staged_seq INTEGER,
    revert_orig_snap  TEXT,
    total_cost_usd    REAL NOT NULL DEFAULT 0.0,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

CREATE TABLE session_message (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    parent_msg_id   TEXT,                 -- Hỗ trợ Session Tree (/tree) dạng DAG!
    seq             INTEGER NOT NULL,
    msg_type        TEXT NOT NULL,        -- user | assistant | synthetic | system | compaction | shell | skill
    model_provider  TEXT,
    model_id        TEXT,
    finish_reason   TEXT,
    error_json      TEXT,
    usage_json      TEXT,
    snapshot_start  TEXT,
    changed_files   TEXT,
    data_json       TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    UNIQUE (session_id, seq)
);
CREATE INDEX idx_msg_session_seq ON session_message(session_id, seq);

CREATE TABLE session_inbox (
    id           TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    item_type    TEXT NOT NULL,           -- prompt | synthetic | compaction | move
    delivery     TEXT NOT NULL,           -- steer | queue
    enqueued_seq INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX idx_inbox_session_seq ON session_inbox(session_id, enqueued_seq);

-- Đóng băng snapshot pruning theo epoch để không phá prompt cache khi Warm (Giải Bẫy 1)
CREATE TABLE context_view_epoch (
    session_id      TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    epoch_id        INTEGER NOT NULL,
    reason          TEXT NOT NULL,        -- cache_cold | soft_shake | break_even | compaction
    pruned_map_json TEXT NOT NULL,        -- map call_id -> placeholder text
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (session_id, epoch_id)
);

CREATE TABLE instruction_blob (
    hash       TEXT PRIMARY KEY,          -- blake3 hex
    value_json TEXT NOT NULL
);

CREATE TABLE instruction_state (
    session_id          TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
    epoch_seq           INTEGER NOT NULL,
    initial_hashes_json TEXT NOT NULL,    -- Render system[1]
    current_hashes_json TEXT NOT NULL     -- So sánh diff tạo <system-update>
);

CREATE TABLE permission_saved (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    action     TEXT NOT NULL,
    resource   TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (project_id, action, resource)
);

CREATE TABLE subagent_outbox (
    id                TEXT PRIMARY KEY,
    parent_session_id TEXT NOT NULL,
    child_session_id  TEXT NOT NULL,
    payload_xml       TEXT NOT NULL,
    delivered         INTEGER NOT NULL DEFAULT 0,
    created_at        INTEGER NOT NULL
);

-- Telemetry bắt buộc cho Eval A/B (Qwen 3.8 vs Claude vs Gemini vs Qwen 4)
CREATE TABLE step_metrics (
    id                  TEXT PRIMARY KEY,
    session_id          TEXT NOT NULL,
    step_num            INTEGER NOT NULL,
    model_profile       TEXT NOT NULL,
    edit_strategy       TEXT NOT NULL,
    non_cached_in       INTEGER NOT NULL,
    cache_read_in       INTEGER NOT NULL,
    cache_write_in      INTEGER NOT NULL,
    output_tokens       INTEGER NOT NULL,
    reasoning_tokens    INTEGER NOT NULL,
    cost_usd            REAL NOT NULL,
    ttft_ms             INTEGER NOT NULL,
    wall_ms             INTEGER NOT NULL,
    prefix_preserved    INTEGER NOT NULL, -- 1 nếu blake3 prefix khớp lượt trước
    edit_attempts       INTEGER NOT NULL DEFAULT 0,
    edit_first_try_ok   INTEGER NOT NULL DEFAULT 0,
    lsp_errors_caught   INTEGER NOT NULL DEFAULT 0,
    raw_tool_bytes      INTEGER NOT NULL DEFAULT 0,
    context_tool_bytes  INTEGER NOT NULL DEFAULT 0,
    created_at          INTEGER NOT NULL
);

CREATE VIRTUAL TABLE session_fts USING fts5(session_id, message_id, content);
```

---

## Phần 4 — Đặc tả Thuật toán & Hằng số Cốt lõi (Algorithmic Invariants)

Implementer AI **PHẢI** tuân thủ chính xác các hằng số và thuật toán sau (không tự sáng tác khác):

1. **Hằng số Vòng lặp, Retry & Ngân sách ngữ cảnh:**
   - `COOPERATIVE_STOP_TIMEOUT = 5s` (user interrupt gọi cancel listener của tool 5s rồi mới kill cứng).
   - `DEFAULT_MAX_RESUME_ATTEMPTS = 10` (chặn crash-loop khi boot).
   - `RETRY_GENERAL`: exponential `2s → 4s → 8s → 10s×7` (tối đa 10 lần, ±20% jitter).
   - `RETRY_NETWORK` (DNS/unreachable, **trừ** `ECONNREFUSED`): exponential `2s → cap 30s`, kéo dài tối đa `1h`, không trừ vào ngân sách `RETRY_GENERAL`.
   - `RETRY_TIMEOUT`: tối đa `3` lần.
   - `RETRY_AFTER_CAP = 15m` (chặn header `Retry-After` độc hại).
   - `OUTPUT_LIMIT_STREAK = 3` (3 assistant liên tiếp có `finish == Length` không có user xen giữa → `StepFailedError::OutputLimit`).
   - `OUTPUT_TOKEN_FALLBACK = 32_000`, `OUTPUT_TOKEN_MAX = 256_000`, `OUTPUT_TOKEN_MIN = 1_024`, `ESTIMATE_ERROR = 1.15`, `WINDOW_HEADROOM = 0.01`.
   - Công thức `output_limit`: `min(requested, max(1024, floor(ctx * (1.0 - headroom)) - measured - ceil(estimated * 1.15)))`.
   - `bound_images` hysteresis: kích hoạt khi tổng base64 ảnh `> 25 MiB`, bỏ ảnh cũ nhất cho tới khi `≤ 15 MiB` (ổn định prefix cache).

2. **Bảng Quyết định `SessionStep::attempt` (16 nhánh theo đúng thứ tự ưu tiên):**
   1. Chưa có output + `AiError::ContextOverflow` + `recover_overflow == true` → `AttemptOutcome::NeedCompaction` (chỉ 1 lần/step).
   2. Stream kết thúc êm nhưng `finish == Unknown` → chuyển thành `AiError::IncompleteStream` (retryable).
   3. Có `AiError` (không phải overflow) → hỏi `RetryPolicy`. Nếu retry được & **chưa có output** → `AttemptOutcome::Retry { fresh_assistant: false }`.
   4. Có lỗi sau output → `fail_assistant(err)`.
   5. Có tool trả `ToolOutcome::UserDeclined` → đánh tool bị từ chối `"The user declined this tool call"`, các tool chưa settle thành `"interrupted"`, assistant thành `"Step interrupted"`, trả `AttemptOutcome::UserAborted` (dừng turn!).
   6. Stream thành công → settle các tool input hỏng (`MalformedJson` / `TruncatedByLength` / `IncompleteStream`, excerpt tối đa `2048` ký tự) thành `ToolResult { is_error: true }`.
   7. Chụp snapshot lần 2 (nếu bật), ghi `Step.Ended` / `Step.Failed` + `step_metrics` trong 1 transaction.
   8. Nếu retry được & **đã có output** (không decline, không interrupt) → `AttemptOutcome::Retry { fresh_assistant: true }` (lượt sau cắt assistant cũ ngay sau tool local cuối đã chạy!).
   9. Nếu `finish == Length` → `AttemptOutcome::OutputLimit { needs_continuation }`.
   10. Ngược lại → `AttemptOutcome::Completed { needs_continuation }`, trong đó `needs_continuation = !ends_turn && tool_choice != None && any_local_tool_called_or_settled`.

3. **Thuật toán `Hashline`, `StrReplace`, `replace_symbol` & `ast_edit` (`harness-edit`):**
   - `ALPHABET = b"ZPMQVRWSNKTXJBYH"` (16×16 = 256 mã, bỏ ký tự dễ nhầm `0/O/1/I/l`).
   - `line_hash(line, prev, next)`: FNV-1a 32-bit trên `line.trim()`. Nếu `norm.len() <= 2` hoặc không có ký tự chữ/số, băm thêm `\x1f + prev.trim() + \x1e + next.trim()`.
   - `apply_hash_edits`: (1) tự strip tiền tố lỡ copy `^\d+#[A-Z]{2}\|` khỏi `content`; (2) xác thực 100% anchor trước (nếu lệch, trả `StaleAnchorError` kèm ±4 dòng thật + hash mới); (3) kiểm tra không chồng lấn; (4) sắp xếp **giảm dần theo số dòng (bottom-up)** và áp dụng nguyên tử trong RAM.
   - `StrReplace`: 3 tầng trên LF view (`exact` → `unicode_normalize` 1-to-1 → `trim_end` theo dòng) + Histogram diff `imara-diff` bảo toàn BOM & CRLF/LF + Hough-voting `missHint` (chỉ dòng distinct, cần `votes * 2 >= wanted`, hiện tối đa 12 dòng).
   - `replace_symbol` & `ast_edit`: tính dải thay thế trên AST trong RAM rồi đi qua đúng `FileWriteCoordinator` chung (1 lần hỏi quyền kèm diff tổng, giữ BOM/`imara-diff`, chạy fast lint `oxc`/`ruff` + LSP sau ghi).

4. **Thuật toán `CacheMeter`, `PruneEpoch`, Observation Masking & Compaction 5 Tầng:**
   - **Write-Time Semantic Condenser:** (a) mô phỏng `\r` giữ khung cuối progress bar; (b) strip ANSI; (c) gộp dòng lặp thành `[repeated ×N]`; (d) bộ lọc chuyên biệt `cargo` / `tsc` / `eslint` / `pytest` / `jest` / `git diff` (bỏ lockfile) / `git log` / `git status` / `ls` / `docker` / `kubectl` + rút gọn mảng JSON lặp; (e) bookend `16 KiB head + 16 KiB tail`, luôn lưu bản thô nén `zstd` ra `<data>/shell/<id>.out.zst` (dọn sau 7 ngày).
   - **`PruneEpoch` & Observation Masking:** Khi `CacheMeter::is_warm() == true` và `estimated < profile.soft_shake_tokens`, **CẤM** thay đổi `context_view_epoch` (chỉ tích lũy `PendingPruneTracker`). Mở `epoch_id + 1` khi: `!is_warm()`, HOẶC `estimated >= soft_shake_tokens`, HOẶC `net_saving_break_even > 0`, HOẶC Compaction boundary — giữ nguyên khung `Reasoning + ToolCall`, che `ToolResult` cũ ngoài 8–10 lượt gần nhất / 25K–40K đuôi.
   - **Compaction 5 tầng:** `Tier 0 Write-Time` → `Tier 1 shake` (Observation Masking + `superseded` → đẩy block nặng ra `local://spill/<id>.zst`; nếu tụt `< 55% ceiling` thì xong!) → `Tier 2 Native` (Anthropic server-side `clear_tool_uses`/compaction, cấu hình tag qua `routes.toml`) → `Tier 3 snapcompact` (opt-in vision feature flag) → `Tier 4 Structured Summary` (Background Precompute ở `ceiling - 60K` khi cache warm; template 7 mục + NUDGE 1 lần; giữ `<recent-context>` 15K–20K + `<subagents>` + `<background-work>`; `SHRINK_STEPS = [0.85, 0.70, 0.50, 0.35]` + 5 mức `flatten_and_drop_oldest`; cuối cùng `advanceEpoch()` gộp delta instructions vào `system[1]`).

5. **4 Bất biến Cross-Model Handoff (`Qwen ↔ Claude ↔ Gemini`):**
   - `Qwen / Claude → Gemini`: gắn `thoughtSignature = "skip_thought_signature_validator"` vào lời gọi tool không chữ ký đầu tiên của lượt assistant lịch sử; tách lượt `user` mới cho `<system-update>` ngay sau `functionResponse`.
   - `Qwen / Gemini → Claude`: chỉ replay `thinking + signature` / `redacted_thinking` khi `same_model == true`.
   - `Claude / Gemini → Qwen`: bỏ toàn bộ metadata provider khác, chỉ gửi `role`, `content`, `tool_calls`, `tool` thuần sạch.
   - `Usage`: Anthropic cộng `input + cache_read + cache_write`; Gemini/Qwen dùng `saturating_sub(cached)` (không bao giờ trừ âm).

---

## Phần 5 — Kế hoạch Thực thi 11 Milestones · 36 Atomic Tasks (`T01` → `T36`)

> **Cách dùng cho Frontier Models:** Mỗi Task dưới đây là một **gói công việc độc lập có ranh giới crate rõ ràng và lệnh nghiệm thu cụ thể**. Các nhóm đánh dấu `[PARALLEL]` có thể giao đồng thời cho nhiều agent trên các git worktree riêng mà không xung đột file!

### Milestone M0 — Workspace Foundation & Event Store (Tuần 1)
- **T01 · Cargo Workspace & Fast Build Config**
  - *Files:* `Cargo.toml`, `.cargo/config.toml`, khung 13 `crates/*/Cargo.toml` + `src/lib.rs`, `profiles/*.toml`, `prompts/*.txt`.
  - *Spec:* Khai báo đúng Dependency Firewall (`cargo tree -p harness-cli` không chứa `harness-core` ngoại trừ binary entrypoint nếu nhúng server; `harness-core` không chứa `tree-sitter`/`ratatui`/`rquickjs`/`tantivy` trực tiếp). Toàn bộ tên crate, đường dẫn (`.harness/`), prompt, và comment tuân thủ Luật #7 Clean-Room.
  - *Verify:* `cargo check --workspace` xanh & thời gian incremental link với `mold` `< 1.5s`.
- **T02 · `harness-schema` Canonical Contracts, `PolicyGuard` & Firewall Traits** *(Depends: T01)*
  - *Files:* `crates/harness-schema/src/{message,event,error,tool,permission,agent,guard,ports}.rs`.
  - *Spec:* Toàn bộ structs/enums ở Phần 3 (bao gồm `trait PolicyGuard` mặc định `NoopPolicyGuard` không chặn chủ đề nào) + `ModelProfile` (hỗ trợ `ModelSelector::{Exact, LatestFlagship, LatestFast}` và `ContextWindow::Auto | Fixed(u64)`) & `RouteDescriptor` deserialize được cả 4 file `profiles/*.toml` (`qwen-3.8-27b`, `qwen-4-upcoming`, `claude-flagship`, `gemini-flagship`).
  - *Verify:* `cargo nextest run -p harness-schema` (round-trip serde JSON + TOML + `Usage::visible_output_tokens` không âm + `PermissionEffect` Ord lattice + `NoopPolicyGuard` trả `Pass`).
- **T03 · SQLite WAL Event Store, Single Writer Thread & Projector** *(Depends: T02)*
  - *Files:* `crates/harness-core/src/store/{migrations/0001_init.sql,writer.rs,bus.rs,projector.rs}`.
  - *Spec:* 1 writer thread chuyên dụng nhận `WriteCmd` qua `mpsc` + `oneshot` reply; `BEGIN IMMEDIATE` ghi `event` (tăng `event_sequence` nguyên tử) + chạy `Projector` đồng bộ + commit hook; chỉ phát `tokio::sync::broadcast` sau khi commit. Ephemeral events (`*Delta`, `ToolProgress`) đi thẳng `broadcast`, không chạm SQLite.
  - *Verify:* Test 32 task Tokio bắn ghi đồng thời vào cùng session → 0 lỗi `SQLITE_BUSY`, `seq` liên tục 1..N không lỗ hổng, replay tạo đúng trạng thái bảng `session`/`session_message`.

### Milestone M1 — Tri-Model LLM Layer `harness-ai` (Tuần 2) `[T05, T06, T07 PARALLEL]`
- **T04 · `SchemaProjector`, `AiError` Classifier, `CachePolicy` & Dynamic `ModelResolver`** *(Depends: T02)*
  - *Files:* `crates/harness-ai/src/{client,schema_projector,cache_policy,error_classifier,catalog}.rs`.
  - *Spec:* `prepare_request` pipeline (sanitize surrogates → sort tools → `SchemaProjector` chuẩn Gemini → `CachePolicy` 3 mode `Inline4` / `QwenSystemTail` / `ImplicitPrefix` / `Implicit`); 10-priority error classifier (~40 regex overflow + loại trừ throttle); **`ModelResolver` trong `catalog.rs`** giải phân giải động `latest_flagship`/`latest_fast` (ưu tiên `--model`/env → `/models` discovery + ETag catalog → embedded fallback, **0 hardcode số đời Claude/Gemini trong Rust**).
  - *Verify:* 22 unit tests phủ mọi loại lỗi (429 vs overflow vs throttle vs 401 vs 413), schema có `$defs`/`$ref`/`allOf`/`additionalProperties` phẳng sạch, `Inline4` không vượt quá 4 `cache_control`, và `ModelResolver` tự chọn đúng model mới nhất khi feed danh sách `/models` giả lập mới.
- **T05 `[PARALLEL]` · Protocol 1: `OpenAIChatCompat` (Qwen 3.8 27B & Qwen 4 sắp ra mắt) + `qwen_guard.rs`** *(Depends: T04)*
  - *Files:* `crates/harness-ai/src/protocols/{openai_chat.rs,qwen_guard.rs}`.
  - *Spec:* SSE parser hỗ trợ `reasoning_content` VÀ `<think>...</think>` lẫn trong `delta.content`, `stream_options.include_usage = true`, `chat_template_kwargs` (`enable_thinking`, `preserve_thinking`), **`thinking-only-guard`** cứu `<tool_call>`/`<function=name><parameter=k>v</parameter></function>` kẹt trong `<think>`, JSON fence/trailing-comma repair, fallback sinh `tool_call_id` nếu trống/trùng, `on_halt()` phát `IncompleteStream`.
  - *Verify:* 8 cassette tests (stream sạch, chunk bị xé đôi, `<tool_call>` kẹt trong `<think>`, XML tool call, JSON có ` ```json `, stream đứt giữa chừng, usage có `cached_tokens`).
- **T06 `[PARALLEL]` · Protocol 2: `AnthropicMessages` (Claude Flagship Mới nhất)** *(Depends: T04)*
  - *Files:* `crates/harness-ai/src/protocols/anthropic.rs`.
  - *Spec:* `/v1/messages` SSE state machine (0 hardcode số phiên bản Claude — đọc năng lực từ `catalog.rs`/`routes.toml`); gắn tối đa 4 `cache_control` (tool cuối → sys[0] → sys[1] → tail msg); `thinking` (`adaptive` + `output_config.effort` hoặc `enabled{budget_tokens}` kẹp `[1024, max_tokens/2]` theo capability), `prefix_mismatch_behavior: "drop_block"`; replay `signature` & `redacted_thinking`; mid-conv `system` hoặc `<system-update>`; hỗ trợ tùy chọn **`defer_loading: true`** (Tool Search) & server-side **`clear_tool_uses`** (type/beta tag đọc từ `routes.toml`); **bắt đúng HTTP 200 `stop_reason: "refusal"` + `stop_details`** chuyển thành `AiError::ContentPolicy` rõ ràng; cộng `input + cache_read + cache_write`.
  - *Verify:* 7 cassette tests (adaptive thinking + signature, tool_use streaming, >4 breakpoints bị kẹp an toàn, HTTP 200 `stop_reason: "refusal"` handling, `clear_tool_uses` request shape, usage calculation).
- **T07 `[PARALLEL]` · Protocol 3: `GeminiGenerateContent` (Gemini Flagship Mới nhất) + `ExternalStdioProtocol`** *(Depends: T04)*
  - *Files:* `crates/harness-ai/src/protocols/{gemini.rs,mod.rs}`.
  - *Spec:* `:streamGenerateContent?alt=sse` (0 hardcode số phiên bản Gemini — dùng `ModelResolver` chọn model mạnh nhất mới nhất); bảo toàn `thoughtSignature` trên mọi part model; gắn `skip_thought_signature_validator` cho tool-call không chữ ký đầu tiên; mở lượt `user` mới cho `<system-update>` nếu trước đó là `functionResponse`; không gắn inline cache breakpoint; **expose cấu hình `safety_settings` từ `ModelProfile` TOML** (hỗ trợ `BLOCK_NONE`/`OFF` tùy cấu hình user); `non_cached = promptTokenCount.saturating_sub(cachedContentTokenCount)`; thêm `ExternalStdioProtocol` escape hatch.
  - *Verify:* 6 cassette tests (thoughtSignature round-trip, unsigned tool call sentinel, functionResponse separation, `safety_settings` serialization, usage subtraction không âm, incomplete stream).

### Milestone M2 — Agent Loop, Concurrency & Crash Recovery (Tuần 3)
- **T08 · `Inbox` (`steer`/`queue`) & `SessionRunCoordinator` (Doorbell)** *(Depends: T03)*
  - *Files:* `crates/harness-core/src/{inbox.rs,coordinator.rs,recovery.rs}`.
  - *Spec:* Admission idempotent theo `client_msg_id`; `promote()` theo đúng thứ tự steer compaction → steer prompts → 1 queue item; `Doorbell` chống race idle; `interrupt(resume: true)`; boot recovery quét `claim_owner`, max 10 lần, `settle_stale_tool_calls`.
  - *Verify:* Stress test 50 lượt wake đồng thời ngay sát ranh giới idle (0 tin nhắn rơi), test `steer` không chen giữa `tool_use`/`tool_result`, test crash-resume dọn tool mồ côi.
- **T09 · `to_llm_messages` (Cross-Model Handoff) & `RequestBuilder`** *(Depends: T04–T07)*
  - *Files:* `crates/harness-core/src/context/{to_llm.rs,request_builder.rs}`.
  - *Spec:* Thực thi 4 bất biến đổi model giữa phiên (`Qwen ↔ Claude ↔ Gemini`); cắt assistant message lỗi sau tool local cuối đã chạy; `output_limit` formula; `bound_images` hysteresis (25→15 MiB); `affinity` từ `parent_id ?? fork_source_id ?? id`.
  - *Verify:* Cassette matrix tests cho 6 cặp chuyển đổi (`Qwen→Gemini`, `Claude→Gemini`, `Qwen→Claude`, `Gemini→Claude`, `Claude→Qwen`, `Gemini→Qwen`) + test cắt assistant lỗi không lặp tool đã chạy.
- **T10 · `SessionRunner` (`drain` → `step` → `attempt`) + 16-Row Decision Table** *(Depends: T08, T09)*
  - *Files:* `crates/harness-core/src/runner/{drain.rs,step.rs,attempt.rs}`.
  - *Spec:* Chạy tool song song bằng `JoinSet` ngay khi `ToolCall` arrive; `CancellationToken` hủy được phần thực thi (cooperative stop 5s) nhưng phần commit event bất khả hủy; thực thi đủ bảng quyết định 16 nhánh; `needs_continuation` dựa trên tool local đã gọi/settled; `max_steps` giữ `tools[]` + `tool_choice="none"`; 3-strike `finish=length`.
  - *Verify:* Integration tests với `MockWireProtocol` phủ đủ: parallel tools, `UserDeclined` dừng turn ngay, `CorrectedError` tiếp tục turn, `finish=length` dừng ở lần thứ 3, `max_steps` giữ nguyên prefix tools.
- **T11 · 3 Retry Budgets & Doom-Loop Detector** *(Depends: T10)*
  - *Files:* `crates/harness-core/src/runner/{retry.rs,doom_loop.rs}`.
  - *Spec:* 3 ngân sách retry độc lập (chung/mạng/timeout); Doom-Loop ring buffer 12 phần tử `blake3(tool, canonical_args)` chèn synthetic warning ở lần trùng thứ 3.
  - *Verify:* Unit tests với `tokio::time::pause()` kiểm chứng backoff timing, network budget tách biệt general budget, và doom-loop chặn đúng ở lần 3.

### Milestone M3 — Edit Engine, Tree-sitter & Core File Tools (Tuần 4) `[T12, T13 PARALLEL]`
- **T12 `[PARALLEL]` · `harness-edit`: `FileWriteCoordinator` × `Hashline`, `StrReplace` & `replace_symbol`** *(Depends: T02)*
  - *Files:* `crates/harness-edit/src/{coordinator,endings,hashline,str_replace,symbol_edit}.rs`.
  - *Spec:* `KeyedMutex` theo canonical path; BOM preserve + **`imara-diff` Histogram Diff** bảo toàn CRLF/LF; `Hashline` (FNV-1a 2-char, neighbor hash cho dòng trống/ngoặc, strip prefix `^\d+#[A-Z]{2}\|`, atomic bottom-up apply, ±4 dòng self-heal); `StrReplace` (3 tầng LF view + Hough-voting `missHint` ≤12 dòng); **`replace_symbol`** thay thân hàm/struct theo ranh giới AST; trả hash mới nếu formatter thay đổi file.
  - *Verify:* 22 property & unit tests (CRLF/LF hỗn hợp, BOM, emoji UTF-8, multi-edit bottom-up, overlap reject, stale anchor snippet, `replace_symbol`, Hough voting sai thụt lề).
- **T13 `[PARALLEL]` · `harness-treesitter`: AST Outline Read, `RepoMap` (PageRank) & `<1ms` Syntax Check** *(Depends: T02)*
  - *Files:* `crates/harness-treesitter/src/{outline.rs,repomap.rs,syntax.rs}`.
  - *Spec:* Feature flags `lang-core` (rust, ts, js, py, go, json, toml) và `lang-all`; xuất Outline kèm số dòng & hashline anchor khi file `>= threshold`; **`RepoMap`** quét đồ thị định nghĩa/tham chiếu symbol + Personalized PageRank xuất bản đồ repo trong 1K–2K token; `check_syntax()` báo `ERROR`/`MISSING` trong `<1ms`.
  - *Verify:* Snapshot tests trên repo mẫu xác nhận Outline giảm ≥80% token, `RepoMap` xếp hạng đúng file trung tâm trong budget 1.500 token, và bắt đúng lỗi thiếu `;`/`}`.
- **T14 · Core Tools (`read` + Delta Re-Read, `edit`, `write`, `grep`, `glob`, `repo_map`) với Ripgrep & Nucleo** *(Depends: T12, T13)*
  - *Files:* `crates/harness-core/src/tools/{read,edit,write,grep,glob,repo_map}.rs`.
  - *Spec:* `read` streaming chunk + offset âm + quá EOF trả 100 dòng cuối + "Did you mean?" dùng **`nucleo-matcher`** + Outline tự động + **Delta Re-Read (`read_diff`)** chỉ trả unified diff/`[Unchanged]` khi đọc lại file trong cùng phiên (0% phá cache!); `grep`/`glob` dùng `ignore` + `grep-searcher` + `grep-regex` + `globset` + `memchr` (đường dẫn tương đối ngắn, cắt cửa sổ 160 ký tự quanh match, nhóm theo file, timeout 30s); tool `repo_map`.
  - *Verify:* Integration tests trên fixture có `.gitignore`, file binary NUL, đọc lại file lần 2 trả diff thay toàn văn, fuzzy "Did you mean?" qua `nucleo`, và grep trên file minified JS.
- **T15 · Permission Policy Lattice & Saved Approvals** *(Depends: T03)*
  - *Files:* `crates/harness-core/src/permission/{policy,service,saved}.rs`.
  - *Spec:* `Rules`/`Override`/`Restrict`/`Fallback`; last-match-wins; `deny > ask > allow`; short-circuit deny; `ignore_case` macOS/Win; `is_action_denied` ẩn tool khỏi catalog; pending `oneshot` channel; `Always` tự duyệt pending khớp; `Reject` hủy cùng message; cấm save `read *` cho `.env` & cấm `sudo *`/`bash -c *`; chế độ `Unattended` (`--auto` / `--non-interactive`).
  - *Verify:* 15 unit tests phủ mọi tầng lattice, batch auto-approve, và fail-closed khi agent không tồn tại.

### Milestone M4 — OS Sandbox Shell & First Headless End-to-End Binary (Tuần 5)
- **T16 · `harness-shell`: `Landlock`/Seatbelt OS Sandbox + Background Timeout Shell** *(Depends: T02, T15)*
  - *Files:* `crates/harness-shell/src/{runner,pty,splitter,sandbox_linux,sandbox_macos}.rs`.
  - *Spec:* `SandboxProfile::ReadOnlyWorkspace` (cho `plan`/`explore`), `WorkspaceWrite` (cho `build`), `Disabled`; áp dụng `landlock` ABI V3 trong `Command::pre_exec` trên Linux (+ `bwrap` tùy chọn); timeout 2m chuyển background job + gửi `<background-result kind="shell">`; `SIGTERM → 3s → SIGKILL`; splitter ~250 dòng + bảng ARITY cho UX hỏi quyền.
  - *Verify:* Integration test thật trên Linux xác nhận `python3 -c "open('x','w')"` bị kernel chặn `EACCES` ở `ReadOnlyWorkspace` và `touch /tmp/escape` bị chặn ở `WorkspaceWrite`.
- **T17 · `harness-cli` Headless Runner (`harness run --auto`)** *(Depends: T10, T14, T16)*
  - *Files:* `crates/harness-cli/src/{main.rs,run.rs}`.
  - *Spec:* Chạy end-to-end 1 prompt với chọn `--profile qwen-3.8-27b | qwen-4-upcoming | claude-flagship | gemini-flagship` (hoặc `--model <id>`), in streaming output + usage + cache hit ratio + `step_metrics`.
  - *Verify:* **Mốc E2E đầu tiên:** Chạy thật 1 task sửa bug trên repo test nhỏ với Qwen / Claude / Gemini (hoặc mock HTTP server) và xác nhận sửa file + chạy test pass.

### Milestone M5 — Token Engine Layer 1 & Delta Instructions (Tuần 6) `[T18, T19, T20 PARALLEL]`
- **T18 `[PARALLEL]` · Write-Time Semantic CLI & JSON Condensers + `zstd` Spill** *(Depends: T02)*
  - *Files:* `crates/harness-token/src/{condenser.rs,truncation.rs}`.
  - *Spec:* Mô phỏng `\r`, strip ANSI, gom dòng lặp `[repeated ×N]`, bộ lọc ngữ nghĩa cho `cargo` / `tsc` / `eslint` / `pytest` / `jest` / `git diff` (bỏ lockfile) / **`git log` (7-char hash + 1 line)** / **`git status` (group by dir)** / **`ls -la` (compact tree)** / **`docker`/`kubectl`** / **JSON array collapse** (50 items → 2 mẫu + `[...48 more]`), head+tail bookend, grapheme-safe `byte_boundary`, spill file giữ 7 ngày nén `zstd`.
  - *Verify:* Snapshot tests trên 7 file log/JSON 5.000 dòng xác nhận nén 60–95%, giữ 100% lỗi & không vỡ UTF-8 tiếng Việt/emoji.
- **T19 `[PARALLEL]` · `CacheMeter`, `PruneEpoch`, Observation Masking & `Superseded` Dedup** *(Depends: T03)*
  - *Files:* `crates/harness-token/src/{estimator.rs,cache_meter.rs,prune.rs}`.
  - *Spec:* `tiktoken-rs` + Anchor estimator; `CacheMeter` theo dõi TTL + break-even formula; **Observation Masking** (giữ nguyên khung `Reasoning + ToolCall`, che `ToolResult` cũ ngoài 8–10 lượt gần nhất / 25K–40K đuôi) + `superseded` read tích lũy trong `PendingPruneTracker`; chỉ mở `context_view_epoch + 1` khi `Cold` / chạm `soft_shake` / vượt break-even / compaction.
  - *Verify:* **Prefix Determinism Test:** Assert 4 lượt liên tiếp khi cache `Warm` có `blake3` tiền tố khớp 100%; lượt 5 hết TTL tự bật `epoch + 1` và mask sạch observations cũ!
- **T20 `[PARALLEL]` · `InstructionState` (`blake3` + `imara-diff`), Lazy Nested `AGENTS.md` & 3 Per-Model Prompts** *(Depends: T03)*
  - *Files:* `crates/harness-core/src/instructions/{state,discovery,agent,rules}.rs`.
  - *Spec:* 2 khối `system[0]` / `system[1]`; nguồn xếp từ chung → riêng (`core/agent` cuối, `core/date` chỉ ngày); file watcher `notify` debounce 100ms; sửa `AGENTS.md` gửi unified diff (`imara-diff`) qua `<system-update>`; lazy nested `AGENTS.md` + Tool-Boundary Conditional Rules (`globs:`); config hỗ trợ `instructions_append` & `system_override`; 3 prompt append (`qwen-3.8`, `claude`, `gemini`) sạch 0 kiểm duyệt chủ đề.
  - *Verify:* Test sửa 1 dòng `AGENTS.md` 5KB tạo delta `< 400 bytes` trong khi `system[1]` bất biến; test chuyển `build ↔ plan` không sửa `system[0..1]`; test `advanceEpoch` gộp sạch delta.

### Milestone M6 — Compaction 5 Tầng, Tri-Model Role Routing, `/btw` & `/tree` (Tuần 7)
- **T21 · `shake` (Tier 1) + 5-Level Reducer & `SHRINK_STEPS` + Opt-in `snapcompact`** *(Depends: T19)*
  - *Files:* `crates/harness-token/src/{shake.rs,reducer.rs,snapcompact.rs}`.
  - *Spec:* `shake` đẩy block nặng ngoài cửa sổ đuôi ra `local://spill/<id>.zst`; `flatten_and_drop_oldest` 5 mức (0..4, không bao giờ bỏ exchange cuối) + `SHRINK_STEPS = [0.85, 0.70, 0.50, 0.35]`; `snapcompact` đặt sau feature flag `snapcompact`.
  - *Verify:* Unit tests xác nhận `shake` hạ context `< 55%` tránh gọi LLM, và reducer luôn vừa `target_tokens` ngay cả với hội thoại 300K token.
- **T22 · Compaction Orchestrator + Background Precompute + Structured Summary 7 Mục** *(Depends: T20, T21)*
  - *Files:* `crates/harness-core/src/compaction/{mod.rs,precompute.rs,summary.rs}`.
  - *Spec:* Template 7 mục + update/merge + NUDGE 1 lần; giữ `<recent-context>` 15K–20K + `<subagents>` + `<background-work>`; Background Precompute ở `ceiling - 60K` khi cache warm (giữ kết quả chưa swap, kiểm `held_index` & đuôi `≤ 40%`, tối đa 2 lỗi); `advanceEpoch` khi hoàn tất.
  - *Verify:* Integration tests với mock LLM (test NUDGE khi thiếu heading, test swap precomputed vs hủy khi history đổi, test overflow retry với `SHRINK_STEPS`).
- **T23 · Tri-Model Role Routing (`primary`/`deep`/`compact`/`smol`/`vision`), `/btw` & `/tree`** *(Depends: T09, T22)*
  - *Files:* `crates/harness-core/src/{routing.rs,sidechat.rs,tree.rs}`.
  - *Spec:* Định tuyến tự động việc phụ sang vai `smol` (Qwen `enable_thinking: false` hoặc model Gemini tier `latest_fast`), compaction/ảnh sang Gemini flagship, deep coding sang Claude flagship; `/btw` mượn prefix cache trả lời ephemeral rồi hủy; `/tree` switch/branch trên `session_message.parent_id`.
  - *Verify:* Tests xác nhận `/btw` không ghi vào bảng `session_message` chính và `/tree` rẽ nhánh giữ nguyên lịch sử 2 nhánh độc lập.

### Milestone M7 — Embedded LSP Diagnostics & Shadow Git Snapshot/Revert (Tuần 8) `[T24, T25 PARALLEL]`
- **T24 `[PARALLEL]` · `harness-lsp`: Lazy LSP Diagnostics & Symbol Navigation** *(Depends: T02, T12)*
  - *Files:* `crates/harness-lsp/src/{manager,client,diagnostics,symbols}.rs`.
  - *Spec:* Tự spawn `rust-analyzer` / `typescript-language-server` / `pyright` / `gopls` lần đầu khi `edit`/`write` chạm file; gửi `didOpen`/`didChange`, debounce 150ms / timeout 1500ms, lọc tối đa 15 `ERROR`, fallback `harness-treesitter` syntax check; cung cấp tool **`symbol_read`** (đọc đúng thân 1 hàm/struct qua LSP documentSymbol + range) & **`find_callers`** (LSP `textDocument/references`).
  - *Verify:* Test với mock JSON-RPC LSP stdio server cho diagnostics, `symbol_read`, `find_callers`, và timeout sạch sau 1500ms.
- **T25 `[PARALLEL]` · Shadow Git Snapshot & 3-Phase Revert (`stage`/`clear`/`commit`)** *(Depends: T03)*
  - *Files:* `crates/harness-core/src/{snapshot.rs,revert.rs}`.
  - *Spec:* Repo snapshot tách riêng tại `<data>/snapshot/<project>/<hash>` không đụng `.git` user, bỏ file untracked `> 2 MiB`; chụp đầu/cuối step song song với LLM request; revert 3 pha (`stage` ẩn lịch sử & khôi phục file, `clear` undo cái undo, `commit` xóa seq cũ khi gửi prompt mới).
  - *Verify:* Test sửa 3 file qua 2 step → `stage` về step 1 → kiểm tra file đĩa khôi phục đúng → `clear` khôi phục lại → `stage` + prompt mới `commit` xóa sạch lịch sử cũ.

### Milestone M8 — Subagents, Skills, MCP & Dual-Purpose `rquickjs` Code Mode / Plugins (Tuần 9–10)
- **T26 · Subagents nền (Depth=1, Concurrency Cap) + Transactional Outbox** *(Depends: T08, T10)*
  - *Files:* `crates/harness-core/src/subagent/{tool.rs,outbox.rs}`.
  - *Spec:* Context con trống, thừa hưởng cwd/permissions, depth mặc định 1, giới hạn `max_concurrent_subagents` (2 cho 1 GPU local), ghi `subagent_outbox` trong cùng tx `Execution.Succeeded`, giao `<background-result kind="subagent">` dạng `steer` + wake cha, phục hồi outbox chưa giao khi reboot.
  - *Verify:* Test cha spawn 2 con song song → giả lập reboot trước khi deliver → boot lại vẫn đánh thức cha và nhận đủ 2 kết quả.
- **T27 · Skills Discovery (`.harness/skills/`) & MCP Client (stdio + StreamableHTTP + OAuth PKCE)** *(Depends: T15, T20)*
  - *Files:* `crates/harness-core/src/{skill,mcp}/mod.rs`.
  - *Spec:* Quét `.harness/skills/` ở thư mục toàn cục và dự án (tối đa 6 tầng, watcher 100ms), phát delta `<available_skills>`, tool `skill(id)`; MCP client hỗ trợ tools/prompts/resources, mặc định định tuyến mọi MCP tool vào Code Mode.
  - *Verify:* Mock MCP stdio server với 50 tools → xác nhận Qwen 27B chỉ thấy trực tiếp ≤ 8 tool lõi + 1 tool `execute`!
- **T28 · `harness-codemode`: `rquickjs` Code Mode `execute` + User Hook Plugins `*.js`** *(Depends: T15, T27)*
  - *Files:* `crates/harness-codemode/src/{runtime,catalog,plugins}.rs`.
  - *Spec:* Nhúng `rquickjs` (giới hạn RAM 64MB, interrupt timeout, `max_tool_calls = 50`, không FS/net ngoài host `tools.ns.fn()` đi qua `PermissionService`); catalog inline ≤ 2.000 token + `search()`; nạp `~/.harness/plugins/*.js` đăng ký hooks (`tool.execute.before/after`, `permission.evaluate`, `session.context`, `session.idle`, `compaction`) + hot-reload.
  - *Verify:* Test script JS gọi `await tools.fs.glob(...)` & lọc mảng; test vòng lặp vô hạn `while(true){}` bị ngắt bởi interrupt handler sau 2s; test plugin `guard.js` chặn `rm -rf /` qua `tool.execute.before`.

### Milestone M9 — Daemon Server, `ratatui` TUI & Eval A/B Suite (Tuần 11–12) `[T29, T31 PARALLEL]`
- **T29 `[PARALLEL]` · `harness-protocol` & `harness-server` (`axum` HTTP/UDS + SSE)** *(Depends: T10, T15)*
  - *Files:* `crates/harness-protocol/src/lib.rs`, `crates/harness-server/src/{main,routes,sse}.rs`.
  - *Spec:* REST API (`POST /sessions`, `POST /sessions/:id/prompt`, `POST /sessions/:id/interrupt`, `POST /permissions/:id/reply`, `GET /events` SSE); hoàn toàn tách biệt UI khỏi `harness-core`.
  - *Verify:* Integration test HTTP client gửi prompt, nhận SSE streaming events và trả lời permission `once`/`always`/`reject`.
- **T30 · `ratatui` TUI Client (Live Cache Meter & Token Savings HUD)** *(Depends: T29)*
  - *Files:* `crates/harness-cli/src/tui/{mod,app,view,permission_modal}.rs`.
  - *Spec:* Kết nối `harness-server` qua HTTP/UDS + SSE (0 phụ thuộc `harness-core`); thanh trạng thái hiển thị trực tiếp: Model đang dùng (`Qwen 3.8 27B` / `Claude` / `Gemini`), **Cache Meter** (`Warm 4m12s · 86% hit`), tỷ lệ nén Write-Time, chi phí tích lũy, hộp thoại Permission (`Allow once / Always [hiện rõ pattern!] / Reject / Reject with feedback`), lệnh `/btw`, `/tree`, `/compact`, `/model`.
  - *Verify:* Headless terminal backend test (`TestBackend` của `ratatui`) render sạch mọi panel & modal không panic.
- **T31 `[PARALLEL]` · `harness eval` A/B Benchmark Runner (`--repeat 3`)** *(Depends: T17, T23)*
  - *Files:* `crates/harness-core/src/eval.rs`, `crates/harness-cli/src/eval_cmd.rs`.
  - *Spec:* Đọc bộ task YAML/JSON (repo, prompt, verify command), chạy trong sandbox cô lập với `--repeat 3` qua nhiều cấu hình (`--profile qwen-3.8-27b,qwen-4-upcoming,claude-flagship,gemini-flagship --edit hashline,str_replace`), đọc bảng `step_metrics` xuất báo cáo so sánh `pass@1`, `cost_per_resolved_task`, `cache_hit_ratio`, `edit_first_try_rate`, `lsp_errors_caught`.
  - *Verify:* Chạy thử trên 3 mini-task mẫu với mock/local model tạo bảng báo cáo Markdown & JSON đầy đủ.
- **T32 · V1 Hardening, Release Binary & Full Eval Baseline** *(Depends: T28, T30, T31)*
  - *Spec:* `cargo build --release --features "lang-core,tui"`, kiểm tra kích thước binary, cold start `< 15ms`, chạy full test suite `cargo nextest run --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`, chạy grep kiểm tra Clean-Room (0 tên dự án bên ngoài & 0 hardcode số đời model Claude/Gemini), lưu baseline Eval V1 cho Qwen 3.8 27B / Qwen 4 / Claude Flagship / Gemini Flagship.

### Milestone M10 — V1.5 Accelerator Pack (`ast-grep`, `gitoxide`, `oxc`/`ruff`, `tantivy`) `[T33–T36 PARALLEL]`
- **T33 `[PARALLEL]` · `ast-grep-core` Structural Search & `ast_edit` Tool** *(Depends: T12, T13)*
  - *Files:* `crates/harness-edit/src/ast_edit.rs`, `crates/harness-core/src/tools/ast_edit.rs`.
  - *Spec:* Tích hợp `ast-grep-core` cho phép tìm và biến đổi đa file theo mẫu AST (`$OBJ.old($$$ARGS)` → `$OBJ.new($$$ARGS)`), chạy qua đúng `FileWriteCoordinator` chung (1 lần hỏi quyền kèm diff tổng, giữ BOM/`imara-diff`, chạy fast lint + LSP sau ghi).
  - *Verify:* Test refactor 10 file Rust & TypeScript có comment/string trùng tên hàm → chỉ sửa đúng AST node gọi hàm, không đụng comment/string.
- **T34 `[PARALLEL]` · `gix` (`gitoxide`) Pure Rust Git Engine** *(Depends: T25)*
  - *Files:* `crates/harness-core/src/git_native.rs` (nâng cấp `snapshot.rs` & git status/diff).
  - *Spec:* Thực hiện đọc status, tính blob diff (`imara-diff` tích hợp trong `gix`), duyệt commit graph và ghi shadow snapshot tree hoàn toàn bằng Pure Rust `gix` (thread-safe, 0 fork process).
  - *Verify:* Benchmark & correctness test so sánh output `gix` khớp 100% trên repo có `.gitignore` lồng nhau và submodule.
- **T35 `[PARALLEL]` · Ultra-Fast Lint (`oxc` + `ruff`) & Zero-Node TypeScript Plugin Stripping** *(Depends: T24, T28)*
  - *Files:* `crates/harness-lsp/src/fast_lint.rs`, `crates/harness-codemode/src/ts_strip.rs`.
  - *Spec:* (1) Nhúng `oxc_parser`/`oxc_linter` kiểm tra lỗi JS/TS `< 5ms` ngay sau `edit`, gọi `ruff check --output-format json` `< 10ms` cho Python; (2) dùng `oxc_transformer` strip TypeScript types `< 1ms` để user thả thẳng file `~/.harness/plugins/*.ts` chạy trong `rquickjs` mà không cần Node/Bun!
  - *Verify:* Test sửa file `.ts` có lỗi cú pháp nhận báo lỗi trong `< 10ms`; test nạp 1 plugin `guard.ts` có TypeScript type annotations chạy mượt trong `rquickjs`.
- **T36 `[PARALLEL]` · `tantivy` BM25 Full-Text Indexer (Monorepo > 10K–50K Files)** *(Depends: T02, T14)*
  - *Files:* `crates/harness-index/src/lib.rs` (sau Cargo feature `monorepo-index`).
  - *Spec:* Tự động xây & cập nhật gia tăng (incremental) chỉ mục BM25 `tantivy` trên đĩa khi workspace vượt ngưỡng `> 10.000` file, cung cấp tìm kiếm xếp hạng độ liên quan nhanh trong vài mili-giây cho cả tool `search_code` lẫn tăng tốc `RepoMap`.
  - *Verify:* Test lập chỉ mục 5.000 file sinh tự động trong `< 1s` và truy vấn top-10 kết quả trong `< 15ms` với incremental update sau khi sửa 1 file.

---

## Phần 6 — Bộ Prompt Mẫu Giao Việc cho Frontier Models & Bản đồ Song Song Hóa

### Prompt 1 — Khởi tạo Session với Frontier Model (Copy-Paste)

```text
Bạn là Principal Rust Systems Architect đang xây dựng dự án độc lập `rust-harness` (Pure Rust Coding Agent Harness hỗ trợ Qwen 3.8 27B & Qwen 4 sắp ra mắt, cùng model flagship mạnh nhất mới nhất của Claude và Gemini).

Hãy đọc kỹ file đặc tả kỹ thuật gốc tại: `/home/nambui/pure-rust-harness-plan.md`

Tuân thủ TUYỆT ĐỐI 8 Luật Thép ở Phần 1:
1. Giữ Dependency Firewall 13 Crates (crate lá nặng chỉ nối qua trait ở `harness-schema`; `harness-cli`/`tui` không import `harness-core` trực tiếp).
2. Dùng `Arc<dyn Trait>` ở tầng điều phối để giữ incremental compile 1-3 giây; CẤM hardcode số phiên bản model của Claude/Gemini trong Rust (giải phân giải động qua `catalog.rs` `latest_flagship`/`latest_fast` + TOML; riêng Qwen giữ `qwen-3.8-27b.toml` & `qwen-4-upcoming.toml`).
3. Dùng `BTreeMap` & sort `tools[]` theo tên để đảm bảo byte-exact prompt prefix cache.
4. Zero `.unwrap()` ở runtime path; `ToolOutcome::UserDeclined` tách riêng không bao giờ biến thành `ToolFailure`.
5. Mọi lệnh ghi SQLite đi qua 1 dedicated WAL writer thread (`BEGIN IMMEDIATE`).
6. Zero Built-in Content Censorship: không nhúng prompt kiểm duyệt chủ đề/đạo đức hay preset SDK bên ngoài; mặc định dùng `NoopPolicyGuard`; xử lý trung lập HTTP 200 `stop_reason: "refusal"` của Anthropic và mở `safety_settings` TOML cho Gemini.
7. Clean-Room & Original Brand Identity (BẮT BUỘC): TUYỆT ĐỐI KHÔNG nhắc tên, trích dẫn hay đặt tên đường dẫn/comment/docstring/prompt/test/commit theo bất kỳ agent hay harness bên ngoài nào; mọi thư mục cấu hình chỉ dùng namespace `.harness/` và tệp hướng dẫn `AGENTS.md`.
8. Mọi Task (`T01`–`T36`) chỉ hoàn thành khi test cassette/unit offline pass `cargo nextest run -p <crate>` và `cargo clippy --workspace --all-targets -- -D warnings` sạch.

Xác nhận đã hiểu kiến trúc rồi thực hiện Task T01 (và dừng lại báo cáo sau khi pass verify).
```

### Prompt 2 — Giao Từng Task Độc Lập (`Txx`) hoặc Chạy Song Song Nhiều Agent

```text
Thực hiện đúng Task [T05] từ `/home/nambui/pure-rust-harness-plan.md`:
- Phạm vi file được phép tạo/sửa: CHỈ các file liệt kê trong Task [T05] (không sửa crate khác trừ khi thật sự thiếu field trong `harness-schema`).
- Bám sát 100% các hằng số, thuật toán ở Phần 3–4 và toàn bộ 8 Luật Thép (đặc biệt Luật #7 Clean-Room: tuyệt đối không nhắc bất kỳ dự án/harness bên ngoài nào trong code, comment hay test).
- Viết đủ toàn bộ unit/cassette tests theo mục Verify của Task [T05].
- Chạy `cargo nextest run -p <crate>` và `cargo clippy -p <crate> --all-targets -- -D warnings`, sửa hết lỗi rồi mới kết thúc và báo cáo kết quả test.
```

### Bản đồ Song Song Hóa (5 Đợt Thực thi)
- **Đợt 1 (tuần tự):** `T01` → `T02` → `T03` + `T04` (song song).
- **Đợt 2 (song song 6 agent):** `T05` (Qwen) ‖ `T06` (Claude) ‖ `T07` (Gemini) ‖ `T12` (`harness-edit`) ‖ `T13` (`harness-treesitter` + `RepoMap`) ‖ `T18` (CLI/JSON Condensers).
- **Đợt 3 (sau Đợt 2):** `T08` + `T09` → `T10` → `T11`, song song `T14` (`read_diff`/Ripgrep/Nucleo), `T15`, `T16`, `T19` (Observation Masking + `PruneEpoch`), `T20`.
- **Đợt 4 (tích hợp & hoàn thiện V1):** `T17` (E2E V1!) → song song `T21–T23` (Compaction/Routing), `T24` (`harness-lsp` + `symbol_read`/`find_callers`), `T25` (Snapshot), `T26–T28` (Subagent/MCP/`rquickjs`), `T29–T31` (Server/TUI/Eval) → `T32` V1 Release!
- **Đợt 5 (V1.5 Accelerator Pack — song song 4 agent):** `T33` (`ast-grep-core` `ast_edit`) ‖ `T34` (`gix` Pure Rust Git) ‖ `T35` (`oxc`+`ruff` lint & `.ts` plugin strip) ‖ `T36` (`tantivy` BM25 indexer)!

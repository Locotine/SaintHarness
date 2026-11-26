# `SaintHarness` CLI & TUI Prototype Specification (Aligned with Saint Hyper Agent Design System)

> **Trạng thái:** Prototype thiết kế dự kiến (chưa implement mã Rust).
> **Bản xem trước tương tác (mở trực tiếp trong trình duyệt hoặc HTML preview của Saint):** [saint-harness-cli-prototype.html](./saint-harness-cli-prototype.html)
> **Kế hoạch triển khai lõi:** [IMPLEMENTATION_PLAN.md](../IMPLEMENTATION_PLAN.md)
> **Nguồn sự thật:** prototype HTML là chuẩn; spec này và [tui-parity-contract.json](./tui-parity-contract.json) ghi lại đúng giá trị trong prototype (đã qua bộ lọc antislop ngày 2026-11-26).
> **Nguồn Design System & CLI đối chiếu:** `Saint Hyper Agent` (`/media/Work/Loco/saint-hyper-agent`: `theme.seed.generated.ts`, `theme.logic.ts`, `index.css`, `terminalRuntimeAppearance.ts`, `apps/server/src/main.ts`, `serverStatusCli.ts`, `KEYBINDINGS.md`, `ComposerPendingApprovalPanel.tsx`, `ComposerChoiceRow.tsx`, `ChatEmptyStateHero.tsx`, `AsciiWaveBackground.tsx`, `ClaudeCacheDetails.tsx`).

---

## 1. Triết lý Giao diện (Agent-Native Workspace × Saint Quiet-Chrome)

> **Design Read:** workspace agent cho developer, ngôn ngữ quiet-chrome của Saint Hyper Agent, dial **ENERGY 1 / RHYTHM 1 / MOTION 1**.

`SaintHarness` kết hợp **trải nghiệm workspace terminal liền mạch theo trạng thái agent** (lấy cảm hứng từ mô hình agent-native terminal workspace: 1 không gian duy nhất với thanh trạng thái bên trái gom theo *Needs you / Working / Ready / Idle*, chuyển tức thì sang *Agent Board*, mọi chi tiết nặng ẩn sau progressive disclosure) với ngôn ngữ thị giác **yên tĩnh, tương phản cao (quiet-chrome)** của `Saint Hyper Agent`:

1. **Liền mạch, không chia mảnh, không nhiễu dashboard:**
   - **Mặc định chỉ có 3 lớp:** (1) Thanh trái mảnh, thu gọn được (`Ctrl+B`), gom các phiên trên daemon SQLite WAL theo `Needs you` · `Working` (spinner braille) · `Ready` · `Idle`; (2) Cột hội thoại ở giữa với khối `▸ Worked for ...` **mặc định thu gọn 1 dòng**; (3) Header 1 dòng chỉ giữ **3 nút nhìn nhanh** (`Warm 4m12s · 88%`, `+24 -6`, `Qwen 3.8`) cộng nhãn `prototype · sample data`.
   - **Progressive disclosure:** Diff Split (`Ctrl+D`), Cache & PruneEpoch popover (bấm `Warm`), chi tiết Landlock sandbox (bấm badge trong thẻ Approval), Agent Board (`Ctrl+G`), ngăn CLI (`Ctrl+J`). Chỉ mở khi cần, `Esc` đóng từng lớp một và trả focus về nút đã mở nó.
   - **`/btw` và `/tree` không có nút ở footer composer** (mọi kích thước màn hình). Gọi qua bảng lệnh `/` hoặc phím `Ctrl+Alt+S` / `Ctrl+T`. Footer chỉ giữ: chế độ Build/Plan, quyền runtime, context, nút gửi.
2. **Nền & lớp phân tầng:** nền chính `#111111` (Dark) / `#ffffff` (Light), sidebar `#141414` / `#fafafa`, composer `#1c1c1c` / `#f3f3f3`, đường phân tách mảnh (`divider` 5.5% ink trên dark, 6.5% trên light), viền thẻ nổi bo tròn `╭─╮│╰─╯`.
3. **Một màu nhấn (`Saint Ember`):** fill `#f2612d` (Dark) / `#c74614` (Light); chữ nhấn `#f6906c` / `#b83f10`. Chỉ dùng cho: viền focus của composer, đường dẫn file bấm được (mở diff), vương miện logo, ripple của sóng ASCII. Không tô màu số thứ tự `[1]`, không tô dòng đang chọn.
4. **Ngữ nghĩa màu: fill tách khỏi chữ.** Màu trạng thái gốc của SHA chỉ dùng làm **fill** (chấm trạng thái, nền dòng diff). Mọi **chữ** mang màu trạng thái dùng biến thể `*_ink` để đạt WCAG AA ≥ 4.5:1 trên mọi nền, kể cả dòng hover và nền tint (giống cách SHA tách `--success` / `--success-foreground`):

   | Ý nghĩa | Fill (Dark / Light) | Chữ `*_ink` (Dark / Light) |
   |---|---|---|
   | Ready / Diff added / Cache Warm | `#00a240` / `#00a240` | `#22b45f` / `#046c4e` |
   | Needs you / Warning | `#f5b44a` / `#d97706` | `#f5b44a` / `#b45309` |
   | Working / Plan mode | `#b06dff` / `#751ed9` | `#c08cff` / `#751ed9` |
   | Error / Diff removed / Decline | `#e02e2a` / `#e02e2a` | `#ef6360` / `#c62521` |
   | Bypass permissions (`full-access`) | `#fe8549` / `#e25505` | `#fe8549` / `#a33a03` |

5. **Thẻ quyết định tách nổi:** **Permission Approval** và **`ask_user`** chỉ hiện khi phiên đang `Needs you`, danh sách `1`–`4` **trung tính hoàn toàn** (số không tô màu; `Decline` chỉ chuyển đỏ khi hover). Giải quyết xong, trạng thái phiên ở thanh trái tự chuyển `Needs you → Working → Ready`.
6. **Không có dấu hiệu "AI slop" (bất biến, hook kiểm):**
   - Không vạch màu bên trái dòng/thẻ; dòng đang chọn = nền đặc + dấu `✓` (hình dạng, không chỉ màu).
   - Không tô màu chữ ở đầu dòng menu (menu quyền, bảng lệnh, `$skill`); chỉ nhãn `full-access` mang màu cảnh báo.
   - Không glow, không nhấp nháy vô hạn; chấm trạng thái phẳng. Sóng ASCII ở landing chạy 5 giây rồi dừng, click mới tạo ripple; `prefers-reduced-motion` / `--no-motion` thì đứng yên.
   - Mỗi dòng menu nằm gọn 1 hàng, không xuống dòng; tên đọc bằng font sans, ID dùng mono và cắt `…` khi thiếu chỗ.
   - Không thanh tiêu đề giả kiểu macOS (traffic lights); cửa sổ host tự vẽ.
   - Không ký tự gạch dài (em dash, U+2014), không emoji trang trí, không từ "seamless".
   - Mọi số liệu trong prototype là **mẫu** và được ghi rõ ("sample data", output eval có dòng `# sample values`).
7. **Model picker dạng cây (cross-model handoff, giữ nguyên phiên):**
   - Dòng lá: `Qwen 3.8 27B`, `Qwen 4`. Dòng provider: `▶ Claude`, `▶ Gemini`; bấm vào thì mở cây và **tải danh sách model từ catalog của provider** (trạng thái `Loading … from the Anthropic/Google catalog…`; daemon offline thì báo lỗi kèm `Retry`).
   - Claude: `Auto` (`latest_flagship`), `Fable 5.2` (`claude-fable-5-2`), `Opus 5.5` (`claude-opus-5-5`), `Sonnet 5.5` (`claude-sonnet-5-5`), `Haiku 5.5` (`claude-haiku-5-5`).
   - Gemini: `Auto` (`latest_flagship`), `Pro` (`latest_family:gemini-pro`), `Flash` (`latest_family:gemini-flash`). ID cụ thể do catalog trả về lúc chạy, harness không hardcode.
   - Chọn một model: nhãn header đổi theo, transcript ghi dòng handoff (`Handoff to Claude Opus 5.5 (claude-opus-5-5) · …`), `✓` đánh dấu model hiện tại khi mở lại.
8. **Trạng thái đầy đủ:** danh sách phiên có loading (skeleton), error (`Can't reach the daemon` + `Retry connection`) và empty; Board có câu trạng thái trống cho từng cột; Diff drawer có trạng thái "No changes in this session yet"; bảng lệnh có "No match"; `server status` có output khi daemon unreachable.
9. **Tương thích Terminal Sáng & `NO_COLOR`:** Tự phát hiện màu nền terminal (`OSC 11`) hoặc cờ `--theme saint-dark | saint-light | auto`, đảm bảo tương phản WCAG AA ≥ 4.5:1 trên nền sáng và hỗ trợ đầy đủ `NO_COLOR=1`.
10. **Khớp 100% Hệ thống Font của Saint Hyper Agent (`index.css`, `fontFamily.ts`, `appSettings.ts`, `chatTypography.ts`, `terminalRuntimeAppearance.ts`):**
   - **UI / Chat (Web/Desktop SHA):** `--font-ui-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", system-ui, sans-serif`, `13px`, `line-height: 1.625`, `letter-spacing: normal` (trên macOS tự dùng SF Pro nguyên bản; không bóp âm tracking).
   - **Code & Terminal mặc định SHA:** `"JetBrains Mono Variable", "JetBrains Mono", "SF Mono", "SFMono-Regular", Consolas, Menlo, monospace` (`12px`, `weight 300/500`), kèm tùy chọn đổi nhanh sang `"SF Mono"` ưu tiên đầu tiên (logo Braille 16×13 chuẩn tỷ lệ 0.60 trên cả `JetBrains Mono` lẫn `SF Mono`).
   - **TUI thật (`ratatui`) & Bản quyền:** Binary Rust **không đóng gói bất kỳ tệp font nào** (0 rủi ro bản quyền Apple SF), tự động dùng font của terminal host (hoặc `--terminal-font-family` khi chạy bên trong terminal tích hợp của SHA; và khi cắm làm Provider trong SHA thì UI chat SHA render trực tiếp bằng font SHA).
11. **Hợp đồng kiểm chuẩn tự động & responsive (`tui-parity-contract.json` + hook `check-prototype-parity.sh`):**
   - Mọi màu RGB (Dark/Light, cả fill lẫn `*_ink`), logo Braille (`16×13` & `10×8`), 5 điểm gãy TUI (`80×24`, `100×30`, `120×38`, `140×42`, `180×50`), điểm gãy web (`1100`, `820`, `640`, `480` px), thứ tự nhóm sidebar (`blocked > working > done > idle`), các bất biến progressive disclosure và bất biến chống slop ở mục 6 được khóa trong [tui-parity-contract.json](./tui-parity-contract.json).
   - Khi triển khai `T00` đến `T30`, hook chạy `ratatui::backend::TestBackend` trên cả 5 kích thước: **lệch màu, tương phản < 4.5:1, tràn khung, vỡ responsive hoặc sai trạng thái mặc định thì hook fail (exit ≠ 0), sửa tới khi khớp 100%.**
   - **Responsive web (khi nhúng vào SHA):** ≤ 820px sidebar thành lớp phủ có scrim, mặc định đóng, `Esc` đóng; diff drawer phủ toàn màn. ≤ 640px ẩn 2 nút so sánh font `SHA`/`JB` (chỉ là công cụ so sánh của prototype). ≤ 480px nút cache chỉ còn chấm trạng thái. Màn cảm ứng: mọi vùng bấm ≥ 44px. Không tràn ngang ở 320px.

### Logo Terminal Chuẩn (Raster từ Saint Mark gốc: Khiên + Dải S + Vương miện)

Hỗ trợ tự chọn theo kích thước & khả năng glyph của terminal (vương miện có thể tô Saint Ember `#f2612d` hoặc monochrome `#fcfcfc`):

- **Chuẩn mặc định (`Braille 16×13`, sắc nét nhất trên Ghostty, iTerm2, Kitty, WezTerm, Alacritty, Saint Terminal):**
```text
     ⡀ ⣰⣆ ⢀          ← Vương miện (#f2612d hoặc #fcfcfc)
  ⠈⢷⣤⣿⠶⠿⠿⠶⣿⣤⡾⠁
   ⠈⠳⢂⣠⣤⣤⣄⡐⠞⠁
⢀⣠⣴⣶⣿⣿⣿⡿⢿⣿⣿⣿⣶⣦⣄⡀
⣿⡟⠉⣱⣿⢋⣴⡶⢶⣦⡝⢿⣎⠉⢻⣿
⣿⡇ ⣿⡇⣿⣏  ⢹⣿⣼⣿ ⢸⣿
⣿⡇ ⠹⣿⣌⠻⢷⢆⣄⡈⠙⠿ ⢸⣿
⣿⡇ ⡀⠈⠛⠿⢂⣬⡛⢿⣦⡀ ⢸⣿
⣿⣇ ⣿⣷⣦⡀⠈⠙⢿⣧⢹⣷ ⣸⣿
⠹⣿⡄⢻⣧⠻⣷⣤⣤⣾⠏⣼⡟⢠⣿⠏
 ⠹⣿⣆⠻⢷⣦⡍⢩⣴⡾⠟⣰⣿⠏
  ⠈⠻⣿⣦⣈⠁⠈⣁⣴⣿⠟⠁
    ⠈⠙⠻⢿⡿⠟⠋⠁
```
- **Fallback không có glyph Braille (`Half-Block ▀▄█ 20×16`)** và **Chế độ terminal thấp `< 28` dòng (`Compact Braille 10×8`)** đều được tích hợp sẵn nút chuyển đổi trực tiếp trong [saint-harness-cli-prototype.html](./saint-harness-cli-prototype.html).

---

## 2. Bảng Ánh xạ Design Tokens (`Saint Hyper Agent` → `ratatui` Rust)

Giá trị lấy đúng từ prototype. Token dạng alpha trong CSS (`ink 62%`, …) được quy ra màu đặc trên nền `surface` để TUI dùng trực tiếp. Ba chỗ **cố ý lệch khỏi SHA** để đạt WCAG AA (đã chốt theo prototype): `ink_tertiary`, `text_accent` (Light) và các biến `*_ink`.

| Token ngữ nghĩa | Dark | `ratatui::Color` (Dark) | Light | `ratatui::Color` (Light) | Ứng dụng trong TUI / CLI |
|---|---|---|---|---|---|
| `surface` | `#111111` | `Rgb(17, 17, 17)` | `#ffffff` | `Rgb(255, 255, 255)` | Nền chính transcript |
| `surface_under` | `#0c0c0c` | `Rgb(12, 12, 12)` | `#f6f6f6` | `Rgb(246, 246, 246)` | Nền ngoài cùng, ngăn CLI |
| `sidebar_bg` | `#141414` | `Rgb(20, 20, 20)` | `#fafafa` | `Rgb(250, 250, 250)` | Thanh trái, header |
| `panel` | `#171717` | `Rgb(23, 23, 23)` | `#f8f8f8` | `Rgb(248, 248, 248)` | Thẻ Approval / `ask_user` |
| `control_surface` | `#1c1c1c` | `Rgb(28, 28, 28)` | `#f3f3f3` | `Rgb(243, 243, 243)` | Khung Composer |
| `elevated_popover` | `#202020` | `Rgb(32, 32, 32)` | `#ffffff` | `Rgb(255, 255, 255)` | Bảng lệnh `/`, Model picker, `/btw`, menu quyền |
| `user_bubble_bg` | `#1c1c1c` (`ink 4.5%`) | `Rgb(28, 28, 28)` | `#f5f5f5` (`ink 4%`) | `Rgb(245, 245, 245)` | Tin nhắn user, inline code, dòng đang chọn trong sidebar |
| `ink_primary` | `#fcfcfc` | `Rgb(252, 252, 252)` | `#0d0d0d` | `Rgb(13, 13, 13)` | Văn bản chính, nút gửi đảo ngược |
| `ink_secondary` | `#a3a3a3` (`ink 62%`) | `Rgb(163, 163, 163)` | `#696969` (`ink 62%`) | `Rgb(105, 105, 105)` | Mô tả phụ, tool note, metadata |
| `ink_tertiary` | `#9a9a9a` | `Rgb(154, 154, 154)` | `#686868` | `Rgb(104, 104, 104)` | Placeholder, nhãn nhóm, ID model, timestamp (≥ 4.5:1 cả trên dòng hover) |
| `border` | `#262626` (`ink 9%`) | `Rgb(38, 38, 38)` | `#eaeaea` (`ink 8.5%`) | `Rgb(234, 234, 234)` | Viền thẻ, đường dọc của cây |
| `divider_hairline` | `#1e1e1e` (`ink 5.5%`) | `Rgb(30, 30, 30)` | `#efefef` (`ink 6.5%`) | `Rgb(239, 239, 239)` | Hairline phân tách header / pane |
| `border_heavy` | `#343434` (`ink 15%`) | `Rgb(52, 52, 52)` | `#dddddd` (`ink 14%`) | `Rgb(221, 221, 221)` | Viền thẻ Approval, composer |
| `accent` (fill) | `#f2612d` | `Rgb(242, 97, 45)` | `#c74614` | `Rgb(199, 70, 20)` | Saint Ember: viền focus composer, vương miện logo, ripple |
| `text_accent` | `#f6906c` | `Rgb(246, 144, 108)` | `#b83f10` | `Rgb(184, 63, 16)` | Đường dẫn file bấm được, inline code |
| `accent_bg` | `#2c1b14` (`accent 12%`) | `Rgb(44, 27, 20)` | `#faeeea` (`accent 9%`) | `Rgb(250, 238, 234)` | Nền dải "Steer queued" |
| `status_done` (fill) | `#00a240` | `Rgb(0, 162, 64)` | `#00a240` | `Rgb(0, 162, 64)` | Chấm Ready, thanh TTL cache |
| `status_done_ink` | `#22b45f` | `Rgb(34, 180, 95)` | `#046c4e` | `Rgb(4, 108, 78)` | Chữ `+N`, dấu `+` diff, "Warm", Auto mode pill |
| `status_blocked` (fill) | `#f5b44a` | `Rgb(245, 180, 74)` | `#d97706` | `Rgb(217, 119, 6)` | Chấm Needs you |
| `status_blocked_ink` | `#f5b44a` | `Rgb(245, 180, 74)` | `#b45309` | `Rgb(180, 83, 9)` | Chữ "1 approval" / "question", cache sắp hết TTL |
| `status_working` (fill) | `#b06dff` | `Rgb(176, 109, 255)` | `#751ed9` | `Rgb(117, 30, 217)` | Spinner, chấm Working |
| `status_working_ink` | `#c08cff` | `Rgb(192, 140, 255)` | `#751ed9` | `Rgb(117, 30, 217)` | Pill Plan mode, nhãn `ask_user` |
| `status_error` (fill) | `#e02e2a` | `Rgb(224, 46, 42)` | `#e02e2a` | `Rgb(224, 46, 42)` | Chấm daemon offline |
| `status_error_ink` | `#ef6360` | `Rgb(239, 99, 96)` | `#c62521` | `Rgb(198, 37, 33)` | Chữ `-N`, dấu `-` diff, lỗi, Decline khi hover |
| `full_access` (fill) | `#fe8549` | `Rgb(254, 133, 73)` | `#e25505` | `Rgb(226, 85, 5)` | Nền tint pill Bypass |
| `full_access_ink` | `#fe8549` | `Rgb(254, 133, 73)` | `#a33a03` | `Rgb(163, 58, 3)` | Nhãn `full-access`, pill `▲ Bypass permissions` |
| `diff_added_bg` | `#0e2e1a` (`green 20%`) | `Rgb(14, 46, 26)` | `#e0f4e8` (`green 12%`) | `Rgb(224, 244, 232)` | Nền dòng thêm trong diff |
| `diff_removed_bg` | `#3a1716` (`red 20%`) | `Rgb(58, 23, 22)` | `#fce8e8` (`red 11%`) | `Rgb(252, 232, 232)` | Nền dòng xóa trong diff |

---

## 3. Cây Lệnh CLI Đồng bộ 1-1 với `@saint/cli` (`saint`)

Khớp chính xác quy ước đặt tên cờ (`--home-dir`, `--host`, `--port`, `--auth-token`, `--json`, cặp cờ phủ định `--no-*`, nhóm `server status`, `mcp ...`) từ `apps/server/src/main.ts` và `serverStatusCli.ts`:

```text
saint-harness [GLOBAL FLAGS] [COMMAND]

KHÔNG CÓ SUBCOMMAND:
  - Nếu stdout là TTY  → mở thẳng giao diện TUI tương tác (tương đương `saint-harness tui`)
  - Nếu stdin là pipe  → chạy headless 1 lượt đọc prompt từ stdin (tương đương `saint-harness run -`)

GLOBAL FLAGS (khớp quy ước SHA):
  --home-dir <PATH>         Thư mục dữ liệu SaintHarness (env: SAINT_HARNESS_HOME)
  --profile <NAME>          qwen-3.8-27b | qwen-4-upcoming | claude-flagship | gemini-flagship
  --model <ID>              Ghi đè model ID cụ thể (bỏ qua selector latest_flagship)
  --theme <MODE>            auto | saint-dark | saint-light | ansi16
  --json                    Xuất JSON chuẩn cho script / CI
  --no-color                Tắt màu ANSI (cũng tôn trọng env NO_COLOR=1)

COMMANDS:
  tui                       Mở giao diện TUI `ratatui` (hỗ trợ `--attach <URL|SOCK>`, `--session <ID>`)
  run [PROMPT|-]            Chạy headless 1 lượt hoặc CI
     --mode <build|plan>
     --runtime-mode <approval-required|auto|full-access>
        • approval-required  Hỏi trước khi sửa file / chạy shell (mặc định interactive)
        • auto (alias: --auto)  Tự duyệt trong workspace dưới Landlock WorkspaceWrite; chỉ hỏi khi rủi ro/ngoài workspace
        • full-access (alias: --bypass-permissions)  Bỏ qua 100% hộp thoại quyền + tắt OS sandbox (cảnh báo cam #fe8549)
     --sandbox <read-only|workspace-write|disabled>
     --edit <hashline|str_replace>
     --stream-json          Xuất NDJSON canonical events từng dòng
     --max-steps <N>
  serve                     Chạy daemon HTTP/UDS + SSE (dùng cho TUI hoặc Saint Hyper Agent sidecar)
     --mode <standalone|managed>     (`managed` tắt internal git snapshot khi SHA quản lý)
     --host <IP>                     Mặc định 127.0.0.1
     --port <PORT>                   Mặc định 3774 (0 = ephemeral port)
     --socket <PATH>                 Unix Domain Socket (Linux/macOS)
     --auth-token <TOKEN>            Bearer token bắt buộc (alias: --token)
     --idle-exit-ms <MS>             Tự thoát khi không còn session hoạt động
  server status             Kiểm tra daemon sẵn sàng (định dạng giống hệt `saint server status`)
     --url <URL> | --socket <PATH>
     --json
  models                    Liệt kê & giải phân giải động catalog (0 hardcode version Claude/Gemini)
     --refresh              Bỏ qua ETag cache gọi trực tiếp /models
     --json
  sessions                  Quản lý phiên SQLite WAL
     list | resume <ID> | fork <ID> [--through <MSG_ID>] | tree <ID> | export <ID>
  eval <SUITE.YAML>         Chạy benchmark A/B `--repeat 3` xuất bảng `step_metrics`
     --profiles <LIST>      Mặc định qwen-3.8-27b,claude-flagship,gemini-flagship
     --edit <LIST>          hashline,str_replace
     --repeat <N>           Mặc định 3
     --out <FILE>
  mcp                       Quản lý MCP servers cục bộ
     list | test <NAME>
  acp                       Chạy cầu nối ACP JSON-RPC 2.0 qua stdio (cho IDE ngoài)

EXIT CODES CHUẨN:
  0   Thành công / Daemon ready
  1   Lỗi runtime / Daemon unreachable / Eval fail
  2   Sai cú pháp CLI / Thiếu flag bắt buộc
  130 Bị hủy bởi người dùng (Ctrl+C hoặc UserDeclined)
```

---

## 4. Bản đồ Phím tắt TUI Đồng bộ `KEYBINDINGS.md` của Saint Hyper Agent

| Phím tắt | Lệnh trong `SaintHarness` TUI | Tương ứng trong `SHA` (`KEYBINDINGS.md`) |
|---|---|---|
| `Shift+Tab` | Đổi Effort (`low → medium → high`) / hoặc `Build ↔ Plan` | `model.effort.next` (`composerFocus`) |
| `Ctrl+Shift+M` | Mở **Model picker dạng cây** (`Qwen 3.8` / `Qwen 4` / `▶ Claude` / `▶ Gemini`); `Enter` trên provider mở cây và tải catalog | `modelPicker.toggle` (`mod+shift+m`) |
| `Ctrl+B` | Thu gọn / mở thanh phiên bên trái (≤ 820px: lớp phủ, `Esc` đóng) | `sidebar.toggle` |
| `Ctrl+G` | Bật / tắt **Agent Board** | Board view |
| `Ctrl+J` | Bật / tắt **ngăn CLI headless** | `terminal.toggle` |
| `Ctrl+N` | Phiên mới (landing) | `thread.new` |
| `↑` / `↓` / `Enter` | Di chuyển và chạy mục trong bảng lệnh `/`, `@`, `$` | Composer menu |
| `Alt+]` / `Alt+[` | Chuyển nhanh profile kế tiếp / trước (in-session handoff!) | `model.next` / `model.previous` |
| `Ctrl+D` | Bật/tắt **Split Diff & Cache Inspector** bên phải | `diff.toggle` (`mod+d`) |
| `Alt+↓` / `Alt+↑` | Nhảy tới hunk diff tiếp theo / trước | `diff.change.next` / `diff.change.previous` |
| `Ctrl+K` | Mở **Command & Session Search Palette** | `sidebar.search` (`ctrl+k`) |
| `Ctrl+P` | Mở **Fuzzy File Finder** (`nucleo-matcher`) chèn `@path` | `search.files` (`mod+p`) |
| `Ctrl+L` | Đưa con trỏ về Composer | `composer.focus.toggle` (`cmd+l`) |
| `Ctrl+Alt+S` | Mở **Sidechat `/btw`** (mượn prefix cache, không ghi lịch sử). Không có nút ở footer; gọi bằng phím này hoặc `/btw` | `sidechat.toggle` (`mod+alt+s`) |
| `Ctrl+T` | Mở **`/tree` Session Branch Picker** (DAG `parent_msg_id`). Không có nút ở footer; gọi bằng phím này hoặc `/tree` | Thread branch switch |
| `Ctrl+O` | Mở rộng / thu gọn khối **"Worked for..."** gần nhất | Activity detail toggle |
| `1` – `4` | Chọn nhanh đáp án trong thẻ nổi **Approval** / **`ask_user`** | Card-local `1-4` (`ComposerChoiceRow`) |
| `Esc` / `Ctrl+C` | Đóng lớp nổi trên cùng (từng lớp một, trả focus về nút mở) / Cooperative interrupt (5s) | Turn interrupt |

---

## 5. Wireframe ASCII/Unicode Dự kiến (6 Màn hình Cốt lõi)

> Mọi con số trong wireframe là **dữ liệu mẫu** để minh họa bố cục, chưa đo.

### Màn hình A: TUI Landing (`saint-harness`)
> Tái hiện `ChatEmptyStateHero.tsx` + `AsciiWaveBackground.tsx`: nền ký tự trôi nhẹ trong 5 giây rồi đứng yên (ripple accent `#f2612d` khi click), biểu tượng Saint căn giữa, tiêu đề **"Let's build"**, tên repo mờ bên dưới và khung Composer nâng nhẹ ở đáy.

```text
 SaintHarness  ·  main                                    ● Daemon 11ms  │  SQLite WAL
───────────────────────────────────────────────────────────────────────────────────────
        a 7 k        .  m Q 2        ⡀ ⣰⣆ ⢀        x 9                 v R 4      .
    .        p 4  S       .       ⠈⢷⣤⣿⠶⠿⠿⠶⣿⣤⡾⠁     r 8 k         e 1       .
                                   ⠈⠳⢂⣠⣤⣤⣄⡐⠞⠁
                                ⢀⣠⣴⣶⣿⣿⣿⡿⢿⣿⣿⣿⣶⣦⣄⡀
                                ⣿⡟⠉⣱⣿⢋⣴⡶⢶⣦⡝⢿⣎⠉⢻⣿
                                ⣿⡇ ⣿⡇⣿⣏  ⢹⣿⣼⣿ ⢸⣿   ← Real Saint Mark
                                ⣿⡇ ⠹⣿⣌⠻⢷⢆⣄⡈⠙⠿ ⢸⣿     (Braille 16×13)
                                ⣿⡇ ⡀⠈⠛⠿⢂⣬⡛⢿⣦⡀ ⢸⣿
                                ⣿⣇ ⣿⣷⣦⡀⠈⠙⢿⣧⢹⣷ ⣸⣿
                                ⠹⣿⡄⢻⣧⠻⣷⣤⣤⣾⠏⣼⡟⢠⣿⠏
                                 ⠹⣿⣆⠻⢷⣦⡍⢩⣴⡾⠟⣰⣿⠏
                                  ⠈⠻⣿⣦⣈⠁⠈⣁⣴⣿⠟⠁
                                    ⠈⠙⠻⢿⡿⠟⠋⠁
                                       Let's build
                                      SaintHarness

     [ Fix SQLITE_BUSY concurrency ]   [ Map architecture ]   [ Compare Hashline eval ]

 ╭───────────────────────────────────────────────────────────────────────────────────╮
 │ Ask anything · / commands · @ files · $ skills                                    │
 │                                                                                   │
 │  Build   ○ Ask for approval ▾                                      0 / 110K   [↑] │
 ╰───────────────────────────────────────────────────────────────────────────────────╯
  Shift+Tab effort  ·  Alt+] switch model  ·  Ctrl+D diff  ·  / commands  ·  ? help
```

### Màn hình B: Phiên đang chạy, "Worked for..." & Live Changes Capsule
> Khớp `ChatTranscriptPane` + `ComposerLiveChangesHeader`: khối hoạt động gọn gàng, thẻ `Changes +24 -6` nổi sát trên Composer, **Cache Meter** hiển thị thời gian Warm còn lại và tỉ lệ hit trực tiếp trên thanh trạng thái.

```text
 SaintHarness  ·  fix/wal-lock  [T03]       ● Warm 4m12s · 88% hit  │  ↓74%  │  $0.014
───────────────────────────────────────────────────────────────────────────────────────

  ╭─────────────────────────────────────────────────────────────────────────────────╮
  │ Fix SQLITE_BUSY under 32 concurrent Tokio writers and add a regression test.    │
  ╰─────────────────────────────────────────────────────────────────────────────────╯

  ▾ Worked for 9.4s · 5 tools                                       [Ctrl+O collapse]
    ├─ repo_map  budget=1500 tok → 14 symbols ranked (11ms)
    ├─ read      crates/saint-harness-core/src/store/writer.rs   outline 184L → 32L
    ├─ read      crates/saint-harness-core/src/store/bus.rs      [Unchanged · delta 0B]
    ├─ edit      writer.rs   hashline · imara-diff   +24 -6      fast_lint 0 err (2ms)
    └─ shell     cargo nextest run -p saint-harness-core         Landlock RW · exit 0

  Switched the dedicated writer thread to `TransactionBehavior::Immediate` with a
  5000ms `busy_timeout`, and verified 32 parallel Tokio tasks write 1,000 contiguous
  events (`seq 1..1000`) with zero `SQLITE_BUSY` errors.

  ╭─ Changes  +24 -6  ·  2 files  ·  Ctrl+D Review ─────────────────────────────────╮
 ╭┴──────────────────────────────────────────────────────────────────────────────────┴╮
 │ Follow-up or steer while running…  (Shift+Enter newline)                           │
 │  Build   ○ Ask for approval ▾                                    48K / 110K   [↑] │
 ╰────────────────────────────────────────────────────────────────────────────────────╯
```

### Màn hình C: Detached Approval Card (khớp `ComposerPendingApprovalPanel.tsx`)
> Thẻ nổi tách biệt ngay trên Composer, danh sách `1` đến `4` trung tính (số không tô màu) theo `ComposerChoiceRow`, hiển thị rõ pattern khi chọn `Always` và cảnh báo phân biệt `Decline` vs `Cancel turn` (`ToolOutcome::UserDeclined`).

```text
  ╭─ Approve this command? ─────────────────────────────────── 1 pending · Sandbox RW ╮
  │                                                                                   │
  │  $ cargo nextest run --workspace --all-targets                                    │
  │  cwd: /media/Work/Loco/SaintHarness   ·   rule: shell -> ask (default policy)     │
  │                                                                                   │
  │  (1)  Approve once                  Allow just this request                       │
  │  (2)  Always allow this session     Save pattern `cargo nextest *`                │
  │  (3)  Decline                       Reject with feedback, let the agent continue  │
  │  (4)  Cancel turn                   Stop the current turn (UserDeclined)          │
  │                                                                                   │
  ╰────────────────────────────────────────────────── Keys 1-4 · ↑↓ select · Esc stop ╯
```

### Màn hình D: Split Diff & Prompt Cache Inspector (`Ctrl+D`)
> Chia đôi 60/40: Trái giữ transcript gọn, Phải hiện `imara-diff` kèm Hashline anchors (`42#ZP|`) phía trên và **Prompt Cache & Token Engine** (khớp `ClaudeCacheDetails.tsx`) phía dưới.

```text
 SaintHarness  ·  fix/wal-lock  │  Split Diff + Cache Inspector          [Ctrl+D close]
──────────────────────────────────────────────────────────┬────────────────────────────
  ▾ Worked for 9.4s · 5 tools                             │ writer.rs  +24 -6  (1/2)
    └─ edit writer.rs (+24 -6)                            ├────────────────────────────
                                                          │ 38#QM│ let tx = conn
  Switched the writer thread to Immediate transactions    │ 39#VR│-  .transaction()?;
  and verified 32 concurrent Tokio writers...             │ 39#KB│+  .transaction_with_
                                                          │ 40#JN│+    behavior(
  ── Cross-Model Handoff ───────────────────────────────  │ 41#SW│+      Immediate
  Qwen 3.8 27B → Claude Flagship  (prefix preserved ✓)    │ 42#ZP│+    )?;
                                                          ├────────────────────────────
                                                          │ Prompt Cache: Likely warm
                                                          │ TTL: 4m 12s / 5m 00s
                                                          │ ██████████████████░░ 84%
                                                          │ Cache read    41.8K tok
                                                          │ Cache write    6.4K tok
                                                          │ Non-cached       820 tok
                                                          │ PruneEpoch    #2 (frozen)
                                                          │ Write-Time    ↓74% (zstd)
──────────────────────────────────────────────────────────┴────────────────────────────
```

### Màn hình E: Headless CLI (`server status`, `run --auto`, `eval --repeat 3`)
> Khớp chính xác định dạng plain-text dễ grep và `--json` của `apps/server/src/serverStatusCli.ts`.

```text
$ saint-harness server status
SaintHarness server: ready
URL: http://127.0.0.1:3774 (UDS: /run/user/1000/saint-harness.sock)
Projection: healthy (SQLite WAL · high_water_seq=1482 · writer_queue=0)
Active sessions: 2 (1 warm cache · 1 idle)
Catalog: qwen3.8-27b · claude-flagship (resolved) · gemini-flagship (resolved)

$ saint-harness run --auto --profile qwen-3.8-27b "Fix SQLite busy lock"
▸ session  01JNH8K2…  profile=qwen-3.8-27b  edit=hashline  sandbox=workspace-write
  tool  repo_map  1500 tok  (11ms)
  tool  edit      crates/saint-harness-core/src/store/writer.rs  +24 -6  fast_lint=ok
  tool  shell     cargo nextest run -p saint-harness-core  exit=0  (1.8s)
✓ done  9.4s  steps=3  in=48.2K (cache 88%)  out=1.4K  saved=74%  cost=$0.014

$ saint-harness eval benchmarks/core.yaml --repeat 3
# sample values to show the layout; no eval has been run yet
Profile           Edit         Pass@1   Cache Hit   1st-Try Edit   Cost/Task   Wall
─────────────────────────────────────────────────────────────────────────────────────
qwen-3.8-27b      hashline     83.3%    89.2%       91.4%          $0.000*     18.4s
qwen-3.8-27b      str_replace  72.2%    88.7%       74.1%          $0.000*     23.1s
claude-flagship   str_replace  94.4%    91.8%       95.0%          $0.142      14.2s
gemini-flagship   hashline     88.9%    84.6%       89.7%          $0.048      11.9s

$ saint-harness server status          # khi daemon không chạy
error: SaintHarness server is not reachable at http://127.0.0.1:3774
hint: start it with `saint-harness server`, or check $XDG_RUNTIME_DIR/saint-harness.sock
```

### Màn hình F: Model picker dạng cây (`Ctrl+Shift+M` hoặc bấm tên model ở header)
> Tên đọc bằng sans, ID bằng mono và cắt `…` khi hẹp. Dòng đang chọn có nền đặc + `✓`, không vạch màu. Provider mở ra thì tải catalog (có trạng thái loading / error + Retry).

```text
 ╭─ Cross-model handoff, keeps this session ──────────────╮
 │  Qwen 3.8 27B                       hashline · local   │
 │  Qwen 4                                latest_family   │
 │ ────────────────────────────────────────────────────── │
 │  ▼ Claude                               Inline4 cache  │
 │  │ Auto  ✓                            latest_flagship  │
 │  │ Fable 5.2                         claude-fable-5-2  │
 │  │ Opus 5.5                           claude-opus-5-5  │
 │  │ Sonnet 5.5                       claude-sonnet-5-5  │
 │  │ Haiku 5.5                         claude-haiku-5-5  │
 │  ▶ Gemini                              implicit cache  │
 ╰────────────────────────────────────────────────────────╯
   (Gemini mở ra: Auto · latest_flagship / Pro · latest_family:gemini-pro /
    Flash · latest_family:gemini-flash; ID cụ thể do catalog trả về lúc chạy)
```

---

## 6. Ghi chú Triển khai Khi Bước Vào `T17`, `T29`, `T30`

- Toàn bộ bảng màu trong Phần 2 sẽ được cài thành 1 module duy nhất `crates/saint-harness-tui/src/theme.rs` gồm `SaintTheme::dark()`, `SaintTheme::light()`, `SaintTheme::ansi16()` và `SaintTheme::no_color()`, **không hardcode màu rải rác trong widgets**. Theme có cả cặp fill và `*_ink`; widget chữ chỉ được đọc `*_ink`.
- `saint-harness-tui` chỉ nói chuyện với `saint-harness-server` qua `saint-harness-protocol` (0 phụ thuộc `saint-harness-core`, giữ đúng Luật Thép #1).
- Khi chạy bên trong `Saint Hyper Agent`, toàn bộ sự kiện `turn.diff.updated`, `usage`, `cache_meter`, `permission.requested`, `user_input.requested` từ `saint-harness-server` ánh xạ trực tiếp sang UI web/desktop của SHA mà không cần dịch lại khái niệm.

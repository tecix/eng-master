# en-Master

Luyện viết tiếng Anh ngay trên văn bản bạn đang soạn trong GNOME: bôi đen, bấm phím tắt, xem bản sửa,
diff và lỗi cần học; mọi lỗi được ghi lại để ôn tập theo spaced repetition.

Dùng **gói ChatGPT của bạn** (không cần API key) qua cơ chế chính thức
[Sign in with ChatGPT – token sharing for open-source apps](https://developers.openai.com/siwc/token-sharing-open-source),
giống cách Plyph làm.

## Cài đặt

```sh
cargo build --release
./target/release/enmaster install          # ~/.local/bin, phím tắt, desktop entry, nhắc ôn 09:30
enmaster login                              # hoặc bấm "Continue with ChatGPT" trong cửa sổ
```

`install --no-timer` bỏ lời nhắc hằng ngày, `install --drill-time 20:00` đổi giờ. `enmaster uninstall` gỡ
mọi thứ trừ cấu hình và nhật ký học. Bản cài cũ tên `english-mastery` được gỡ tự động; cấu hình, nhật ký học và
kết nối ChatGPT được chuyển sang tên mới.

`enmaster login --register` đăng ký lại app với ChatGPT (để mục ứng dụng đã kết nối trong ChatGPT hiện tên mới).

## Dùng

Phím tắt toàn hệ thống (GNOME):

| Phím | Việc |
|---|---|
| **Super+Alt+C** | Coach ngay văn bản đang bôi đen |
| **Super+Alt+E** | Mở en-Master trên văn bản đang bôi đen để chọn action |
| **Super+Alt+D** | Daily drill |

Trong cửa sổ, mọi thứ làm được bằng bàn phím; mỗi nút đều hiện phím tắt của nó, **F1** mở bảng tra cứu đầy đủ:

| Phím | Việc |
|---|---|
| Alt+1…9 | Chạy action (Coach, Client email, Chat, Social post, Say it natively, VN → EN, action riêng) |
| Ctrl+K, ← →, Enter | Chọn action bằng phím mũi tên |
| Ctrl+Enter | Chạy Coach; khi đã có kết quả thì **Replace** (dán vào app cũ qua `ydotool`) |
| Ctrl+R | Chạy lại action vừa dùng |
| Ctrl+E | Sửa văn bản gốc |
| Ctrl+D | Chuyển Result / Changes (diff) |
| Ctrl+Shift+C | Copy kết quả |
| Esc | Huỷ yêu cầu đang chạy; nếu không có thì đóng cửa sổ |
| Ctrl+P / Ctrl+T | Progress / Daily drill |
| Ctrl+, | Mở file cấu hình |
| Ctrl+Q | Tắt hẳn en-Master (instance chạy nền) |

Drill: Enter để kiểm tra rồi sang thẻ tiếp, Alt+R ("I was right") khi máy chấm sai nhưng bạn đúng.
Progress: Alt+1/2/3 chọn 7/30/90 ngày, Ctrl+S xuất Anki.

Không có gì được bôi đen thì app lấy clipboard, hoặc bạn gõ thẳng vào ô. Sau khi có kết quả, con trỏ nằm sẵn trong
ô Result để bạn chỉnh câu chữ, rồi Ctrl+Enter để gửi.

Dòng lệnh: `enmaster run email < draft.txt`, `report 30`, `anki ~/deck.txt`, `models`, `status`, `help`.

## Cấu hình

`~/.config/enmaster/config.toml` (Ctrl+, hoặc Menu ☰ → Settings File): model, reasoning effort, trình độ, ngôn ngữ
giải thích, **personal context** (bạn là ai, viết cho ai), lệnh dán, action riêng (`[[actions]]`).

## Dữ liệu & quyền riêng tư

- Token ChatGPT: GNOME Keyring (mục "en-Master — ChatGPT connection").
- Nhật ký học: `~/.local/share/enmaster/log.jsonl` (quyền 0600). Mặc định chỉ lưu lỗi và cụm từ,
  không lưu toàn văn email; bật `log_full_text = true` nếu muốn.
- Văn bản chỉ được gửi tới OpenAI khi bạn chạy một action, với `store: false`. Lượt dùng tính vào hạn mức gói
  ChatGPT; đặt giới hạn riêng cho app ở [Manage usage](https://chatgpt.com/settings/usage).

## Cách hoạt động

- Đọc selection: Mutter phản chiếu PRIMARY selection của Wayland sang XWayland, nên `xsel` đọc được mà không
  cần focus (fallback `xclip`, `wl-paste`).
- App chạy thường trú sau lần mở đầu (GApplication một instance), nên lần bấm sau mở ngay và clipboard vẫn
  giữ sau khi đóng cửa sổ. `enmaster quit` để tắt.
- Đăng nhập: `dynamic_agent_client` + `ext_agent_host_id` ở lần đầu, lưu `oaiapp_…` client ID được cấp; PKCE,
  state, nonce, callback `127.0.0.1`; refresh token xoay vòng, khoá file để không đua giữa các tiến trình.
- Gọi `POST /v1/responses` (stream, `store: false`) với structured output JSON; nếu endpoint từ chối
  `text.format` hoặc `reasoning` thì tự thử lại không có trường đó và ghi nhớ.

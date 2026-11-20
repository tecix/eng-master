# en-Master

Luyện viết tiếng Anh ngay trên văn bản bạn soạn trong GNOME: bôi đen, bấm phím tắt, xem bản sửa, các
thay đổi theo từng từ và lời giải thích cho từng lỗi. Mọi lỗi được ghi lại để làm báo cáo tiến độ, ôn
tập theo spaced repetition và xuất sang Anki.

Dùng **gói ChatGPT của bạn** (không cần API key) qua cơ chế chính thức
[Sign in with ChatGPT cho app mã nguồn mở](https://developers.openai.com/siwc/token-sharing-open-source).

[English](README.md)

## Cài đặt

Flatpak và cài từ mã nguồn: xem [README.md](README.md#install). Nhanh nhất khi không dùng Flatpak:

```sh
cargo build --release
./target/release/enmaster install
```

## Phím tắt

| Ở bất kỳ đâu | |
|---|---|
| **Super+Alt+C** | Sửa lỗi văn bản đang chọn |
| **Super+Alt+E** | Mở en-Master với văn bản đang chọn để chọn thao tác |
| **Super+Alt+D** | Ôn tập hằng ngày |

Trong cửa sổ:

| Phím | Việc |
|---|---|
| Alt+1…9 | Chạy thao tác (Sửa lỗi, Email khách hàng, Tin nhắn, Bài đăng, Nói như người bản xứ, Việt sang Anh, thao tác riêng) |
| Ctrl+K, ← →, Enter | Chọn thao tác bằng phím mũi tên |
| Ctrl+Enter | Sửa lỗi; khi đã có kết quả thì **Thay thế** (dán vào app cũ) |
| Ctrl+R | Chạy lại thao tác vừa dùng |
| Ctrl+E | Sửa văn bản gốc |
| Ctrl+D | Chuyển Kết quả / Thay đổi |
| Ctrl+Shift+C | Sao chép kết quả |
| Esc | Huỷ yêu cầu đang chạy, hoặc đóng cửa sổ |
| Ctrl+P / Ctrl+T | Tiến độ / Ôn tập hằng ngày |
| Ctrl+, | Tuỳ chỉnh |
| F1 | Tất cả phím tắt |
| Ctrl+Q | Thoát hẳn (cả phần chạy nền) |

## Tuỳ chỉnh

Ctrl+, mở hộp Tuỳ chỉnh: mô hình, mức suy luận, trình độ, ngôn ngữ giải thích, thông tin về bạn, tự
động dán, lưu toàn văn, số thẻ mỗi lần ôn, giờ nhắc, tài khoản ChatGPT, phím tắt. Thao tác riêng
(`[[actions]]`) vẫn khai báo trong `~/.config/enmaster/config.toml`.

`enmaster login --register` đăng ký lại app với ChatGPT (để mục ứng dụng đã kết nối hiện tên mới).

## Dữ liệu & quyền riêng tư

- Kết nối ChatGPT: GNOME Keyring.
- Nhật ký học: `~/.local/share/enmaster/log.jsonl` (quyền 0600); mặc định chỉ lưu lỗi và cụm từ.
- Văn bản chỉ gửi tới OpenAI khi bạn chạy một thao tác, với `store: false`, và tính vào hạn mức gói
  ChatGPT ([Quản lý lượt dùng](https://chatgpt.com/settings/usage)).

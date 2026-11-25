# Termax error codes · Mã lỗi Termax

Every session failure carries a stable `TMX-<AREA>-NNN` code. The app shows the title,
message and suggested actions inline (never in a modal dialog); this page explains each
code. Codes are never renumbered or reused.

Mỗi lỗi phiên mang một mã ổn định dạng `TMX-<AREA>-NNN`. Ứng dụng hiển thị tiêu đề, thông
báo và hành động gợi ý ngay trong giao diện (không dùng hộp thoại modal); trang này giải
thích từng mã. Mã không bao giờ được đánh số lại hay dùng lại.

The catalogue lives in `crates/termax-session/src/codes.rs`; a unit test checks that every
code there has a section here with matching EN and VI titles. Control-endpoint codes
(`TMX-CTL-*`, returned to `tmx` and MCP clients) live in `crates/termax-control/src/error.rs`
and are checked the same way.

Areas · Nhóm: `NET` Network · Mạng, `DNS` Name resolution · Phân giải tên, `HOSTKEY` Host keys · Khóa máy chủ, `AUTH` Authentication · Xác thực, `SSH` SSH protocol · Giao thức SSH, `CFG` Configuration · Cấu hình, `FWD` Port forwarding · Chuyển tiếp cổng, `CTL` Local control endpoint · Điểm điều khiển cục bộ.

## TMX-NET-001

**EN:** Connection timed out. The server did not answer in time.

**VI:** Hết thời gian kết nối. Máy chủ không phản hồi kịp thời.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Try again · Thử lại
- Check the network, VPN or proxy · Kiểm tra mạng, VPN hoặc proxy
- Check the host address and port · Kiểm tra địa chỉ và cổng của máy chủ

## TMX-NET-002

**EN:** Connection refused. The server refused the connection; no SSH service is listening on that port.

**VI:** Kết nối bị từ chối. Máy chủ từ chối kết nối; không có dịch vụ SSH nào lắng nghe trên cổng đó.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Check the host address and port · Kiểm tra địa chỉ và cổng của máy chủ
- Try again · Thử lại
- Contact the server administrator · Liên hệ quản trị viên máy chủ

## TMX-NET-003

**EN:** Host unreachable. There is no network route to the host.

**VI:** Không thể truy cập máy chủ. Không có đường mạng đến máy chủ.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Check the network, VPN or proxy · Kiểm tra mạng, VPN hoặc proxy
- Check the host address and port · Kiểm tra địa chỉ và cổng của máy chủ
- Try again · Thử lại

## TMX-NET-004

**EN:** Connection lost. The connection to the server was interrupted.

**VI:** Mất kết nối. Kết nối đến máy chủ bị gián đoạn.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Try again · Thử lại
- Check the network, VPN or proxy · Kiểm tra mạng, VPN hoặc proxy

## TMX-NET-005

**EN:** Server not responding. The server stopped answering keepalive messages.

**VI:** Máy chủ không phản hồi. Máy chủ đã ngừng trả lời các gói keepalive.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Try again · Thử lại
- Check the network, VPN or proxy · Kiểm tra mạng, VPN hoặc proxy

## TMX-NET-006

**EN:** Disconnected by server. The server closed the connection.

**VI:** Máy chủ đã ngắt kết nối. Máy chủ đã đóng kết nối.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Try again · Thử lại
- Contact the server administrator · Liên hệ quản trị viên máy chủ

## TMX-DNS-001

**EN:** Name lookup timed out. The DNS resolver did not answer in time.

**VI:** Hết thời gian phân giải tên. Trình phân giải DNS không phản hồi kịp thời.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Check the network, VPN or proxy · Kiểm tra mạng, VPN hoặc proxy
- Try again · Thử lại

## TMX-DNS-002

**EN:** Host name not found. The host name could not be resolved to an address.

**VI:** Không tìm thấy tên máy chủ. Không thể phân giải tên máy chủ thành địa chỉ.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Check the host address and port · Kiểm tra địa chỉ và cổng của máy chủ
- Check the network, VPN or proxy · Kiểm tra mạng, VPN hoặc proxy

## TMX-HOSTKEY-001

**EN:** Host key changed. The server presented a different key than the trusted one. This can mean an attack; the connection was stopped.

**VI:** Khóa máy chủ đã thay đổi. Máy chủ đưa ra khóa khác với khóa đã tin cậy. Đây có thể là một cuộc tấn công; kết nối đã bị dừng.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Review the host key · Xem lại khóa máy chủ
- Contact the server administrator · Liên hệ quản trị viên máy chủ

## TMX-HOSTKEY-002

**EN:** Host key revoked. The server presented a key that has been revoked.

**VI:** Khóa máy chủ đã bị thu hồi. Máy chủ đưa ra một khóa đã bị thu hồi.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Review the host key · Xem lại khóa máy chủ
- Contact the server administrator · Liên hệ quản trị viên máy chủ

## TMX-HOSTKEY-003

**EN:** Host key not trusted. The unknown host key was rejected.

**VI:** Khóa máy chủ không được tin cậy. Khóa máy chủ chưa biết đã bị từ chối.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Review the host key · Xem lại khóa máy chủ

## TMX-HOSTKEY-004

**EN:** Known hosts unavailable. The trusted host keys could not be read or saved, so the server could not be verified.

**VI:** Không đọc được danh sách máy chủ tin cậy. Không thể đọc hoặc lưu các khóa máy chủ tin cậy nên không xác minh được máy chủ.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Try again · Thử lại
- Edit the connection settings · Sửa cài đặt kết nối

## TMX-AUTH-001

**EN:** Authentication failed. The server rejected every configured sign-in method.

**VI:** Xác thực thất bại. Máy chủ từ chối mọi phương thức đăng nhập đã cấu hình.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Check the user name and keys · Kiểm tra tên người dùng và khóa
- Edit the connection settings · Sửa cài đặt kết nối

## TMX-AUTH-002

**EN:** Server skipped authentication. The server accepted the login without checking any credential, which an impostor could do; the connection was refused.

**VI:** Máy chủ bỏ qua xác thực. Máy chủ chấp nhận đăng nhập mà không kiểm tra thông tin xác thực nào, điều mà kẻ giả mạo có thể làm; kết nối đã bị từ chối.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Contact the server administrator · Liên hệ quản trị viên máy chủ
- Edit the connection settings · Sửa cài đặt kết nối

## TMX-AUTH-003

**EN:** Key cannot be used. A private key could not be read or decrypted.

**VI:** Không dùng được khóa. Không thể đọc hoặc giải mã khóa riêng tư.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Check the user name and keys · Kiểm tra tên người dùng và khóa
- Edit the connection settings · Sửa cài đặt kết nối

## TMX-AUTH-004

**EN:** Sign-in cancelled. A required prompt was cancelled.

**VI:** Đã hủy đăng nhập. Một lời nhắc bắt buộc đã bị hủy.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Try again · Thử lại

## TMX-SSH-001

**EN:** SSH protocol error. The SSH connection failed because of a protocol or transport error.

**VI:** Lỗi giao thức SSH. Kết nối SSH thất bại do lỗi giao thức hoặc lỗi truyền tải.

Automatic reconnect · Tự động kết nối lại: yes, with backoff · có, với thời gian chờ tăng dần

Suggested actions · Hành động gợi ý:

- Try again · Thử lại
- Contact the server administrator · Liên hệ quản trị viên máy chủ

## TMX-SSH-002

**EN:** Server refused the session. The server would not open a terminal or channel, for example because of a session limit.

**VI:** Máy chủ từ chối phiên. Máy chủ không mở terminal hoặc kênh, ví dụ do giới hạn số phiên.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Contact the server administrator · Liên hệ quản trị viên máy chủ
- Try again · Thử lại

## TMX-SSH-003

**EN:** Not supported. The operation is not supported on this connection.

**VI:** Không được hỗ trợ. Thao tác này không được hỗ trợ trên kết nối này.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Edit the connection settings · Sửa cài đặt kết nối

## TMX-SSH-004

**EN:** Unexpected SSH error. The SSH layer reported an error Termax does not recognise.

**VI:** Lỗi SSH không xác định. Lớp SSH báo một lỗi mà Termax không nhận diện được.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Try again · Thử lại
- Contact the server administrator · Liên hệ quản trị viên máy chủ

## TMX-CFG-001

**EN:** Invalid connection settings. The connection settings are incomplete or out of range.

**VI:** Cài đặt kết nối không hợp lệ. Cài đặt kết nối chưa đầy đủ hoặc nằm ngoài phạm vi cho phép.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Edit the connection settings · Sửa cài đặt kết nối

## TMX-FWD-001

**EN:** Port forward cannot listen. The local port for a forward could not be opened, for example because it is in use.

**VI:** Không mở được cổng chuyển tiếp. Không thể mở cổng cục bộ cho chuyển tiếp, ví dụ vì cổng đang được dùng.

Automatic reconnect · Tự động kết nối lại: no · không

Suggested actions · Hành động gợi ý:

- Edit the connection settings · Sửa cài đặt kết nối

## TMX-CTL-400

**EN:** Bad control request. The request was malformed or used an unsupported API version.

**VI:** Yêu cầu điều khiển không hợp lệ. Yêu cầu sai định dạng hoặc dùng phiên bản API không được hỗ trợ.

Suggested actions · Hành động gợi ý:

- Update tmx to the same version as the app · Cập nhật tmx cùng phiên bản với ứng dụng

## TMX-CTL-401

**EN:** Client not paired. The client has no valid token: it never paired, its token was revoked, or pairing was denied.

**VI:** Client chưa được ghép cặp. Client không có token hợp lệ: chưa từng ghép cặp, token đã bị thu hồi hoặc yêu cầu ghép cặp bị từ chối.

Suggested actions · Hành động gợi ý:

- Pair again with `tmx pair` and approve the request · Ghép cặp lại bằng `tmx pair` và chấp thuận yêu cầu

## TMX-CTL-403

**EN:** Not allowed for this client. The client's scope or the command policy does not allow this call. When policy allows, an approval request was created.

**VI:** Client này không được phép. Phạm vi quyền của client hoặc chính sách lệnh không cho phép thao tác này. Nếu chính sách cho phép, một yêu cầu phê duyệt đã được tạo.

Suggested actions · Hành động gợi ý:

- Approve the request in Termax · Phê duyệt yêu cầu trong Termax
- Pair again with the operate scope · Ghép cặp lại với quyền operate

## TMX-CTL-404

**EN:** No such session. The session does not exist or has closed.

**VI:** Không có phiên này. Phiên không tồn tại hoặc đã đóng.

Suggested actions · Hành động gợi ý:

- List sessions with `tmx sessions list` · Liệt kê phiên bằng `tmx sessions list`

## TMX-CTL-429

**EN:** Too many control requests. The client sent requests faster than the endpoint's rate limit.

**VI:** Quá nhiều yêu cầu điều khiển. Client gửi yêu cầu nhanh hơn giới hạn của điểm điều khiển.

Suggested actions · Hành động gợi ý:

- Try again · Thử lại

## TMX-CTL-500

**EN:** Control operation failed. The runtime could not carry out the request; the session's own error code, when there is one, is included.

**VI:** Thao tác điều khiển thất bại. Runtime không thực hiện được yêu cầu; mã lỗi của phiên (nếu có) được kèm theo.

Suggested actions · Hành động gợi ý:

- Try again · Thử lại

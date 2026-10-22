import SwiftUI

struct NativeCredential: Codable, Identifiable {
    var host: [UInt8]
    var mode: String
    var value: String
    var remember: Bool
    var id: String { mode }
    var username: String {
        get { systemAccount["username"] ?? "" }
        set { setSystemAccount(username: newValue, password: password) }
    }
    var password: String {
        get { mode == "system_account" ? systemAccount["password"] ?? "" : value }
        set {
            if mode == "system_account" { setSystemAccount(username: username, password: newValue) }
            else { value = newValue }
        }
    }
    private var systemAccount: [String: String] {
        (try? JSONDecoder().decode([String: String].self, from: Data(value.utf8))) ?? [:]
    }
    private mutating func setSystemAccount(username: String, password: String) {
        if let data = try? JSONEncoder().encode(["username": username, "password": password]) {
            value = String(decoding: data, as: UTF8.self)
        }
    }
}

@MainActor
extension ControllerModel {
    private func credentialCall(_ command: [String: Any]) async throws -> Data {
        guard let path = credentialStoragePath else { throw MobileError.message("Controller unavailable") }
        let input = String(decoding: try JSONSerialization.data(withJSONObject: command), as: UTF8.self)
        let response = try await Task.detached(priority: .userInitiated) {
            guard let output = rm_native_credentials(path, input) else { throw MobileError.message("Keychain unavailable") }
            defer { rm_string_free(output) }
            return Data(String(cString: output).utf8)
        }.value
        guard let result = try JSONSerialization.jsonObject(with: response) as? [String: Any],
              result["ok"] as? Bool == true else {
            throw MobileError.message(L("Could not access saved credentials. Check Keychain access and reopen this connection.", "无法访问已保存的凭据，请检查钥匙串权限并重新打开此连接。"))
        }
        return try JSONSerialization.data(withJSONObject: result["value"] ?? NSNull(), options: .fragmentsAllowed)
    }
    func loadNativeCredentials(_ draft: ConnectionDraft) async throws -> [NativeCredential] {
        let data = try await credentialCall(["op":"load", "request":try draft.request(audio:false, clipboard:false)])
        return try JSONDecoder().decode([NativeCredential].self, from:data)
    }
    func saveNativeCredentials(_ draft: ConnectionDraft) async throws {
        guard !draft.nativeCredentials.isEmpty, let original = draft.nativeCredentialRequest else { return }
        let edits = try JSONSerialization.jsonObject(with:JSONEncoder().encode(draft.nativeCredentials))
        _ = try await credentialCall(["op":"save", "request":try draft.request(audio:false, clipboard:false),
                                      "original_request":original, "edits":edits])
    }
}

struct NativeCredentialFields: View {
    @Binding var credentials: [NativeCredential]
    var body: some View {
        ForEach($credentials) { $credential in
            Section {
                Toggle(L("Store in Keychain", "存入钥匙串"), isOn: $credential.remember)
                    .accessibilityIdentifier("nativeRemember-\(credential.mode)")
                if credential.remember {
                    if credential.mode == "system_account" {
                        TextField(L("System username", "系统用户名"), text: $credential.username)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().textContentType(.username)
                            .accessibilityIdentifier("nativeUsername")
                    }
                    SecureField(credential.mode == "system_account" ? L("Login password", "登录密码") : L("Access password", "访问口令"), text: $credential.password)
                        .textContentType(.password).accessibilityIdentifier("nativePassword-\(credential.mode)")
                }
            } header: {
                Text(credential.mode == "system_account" ? L("System account", "系统账户") : L("Access password", "访问口令"))
            } footer: {
                Text(L("Changes apply when you save or connect. Credentials stay on this device.", "保存或连接后生效，凭据仅保存在此设备。"))
            }
        }
    }
}

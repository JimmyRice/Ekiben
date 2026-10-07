// A minimal ticket gate: verifies one ticket against one trusted key.
//
//     verify <public-key-hex> <ticket-hex> <now>
//
// `now` is Unix seconds. Prints the ticket id and exits 0 when the ticket is authentic and
// currently valid, otherwise prints the rejection (e.g. "ピンポーン🔔 BadSignature") and exits 1.
import Foundation

func bytes(hex: String) -> [UInt8]? {
    guard hex.count % 2 == 0 else { return nil }
    var out: [UInt8] = []
    var index = hex.startIndex
    while index < hex.endIndex {
        let next = hex.index(index, offsetBy: 2)
        guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
        out.append(byte)
        index = next
    }
    return out
}

let arguments = CommandLine.arguments
guard arguments.count == 4, let key = bytes(hex: arguments[1]),
    let ticket = bytes(hex: arguments[2]), let now = UInt64(arguments[3])
else {
    FileHandle.standardError.write(Data("usage: verify <public-key-hex> <ticket-hex> <now>\n".utf8))
    exit(2)
}

do {
    let verifier = try Verifier(publicKeys: [Data(key)])
    let claims = try verifier.verify(ticket: Data(ticket), now: now)
    var line = "OK ticket \(claims.ticketId) issued by \(claims.issuer)"
    if let zone = claims.extensions.first(where: { $0.tag == 0x80 }) {
        line += " zone \(String(decoding: zone.value, as: UTF8.self))"
    }
    print(line)
} catch {
    print(error)
    exit(1)
}

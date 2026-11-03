// Generates all platform icons from the approved terminal artwork.
// Usage: swift assets/branding/icon.swift assets/branding/moth.icns

import AppKit

let source = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    .appendingPathComponent("moth-terminal-transparent.png")
guard let artwork = NSImage(contentsOf: source) else {
    fatalError("Cannot load icon artwork at \(source.path)")
}

func drawIcon(size: Int) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSGraphicsContext.current!.imageInterpolation = .high
    artwork.draw(in: NSRect(x: 0, y: 0, width: size, height: size),
                 from: .zero, operation: .copy, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    return rep
}

let output = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "moth.icns")
let iconset = FileManager.default.temporaryDirectory.appendingPathComponent("moth-\(ProcessInfo.processInfo.processIdentifier).iconset")
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

for points in [16, 32, 128, 256, 512] {
    for factor in [1, 2] {
        let name = factor == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
        let png = drawIcon(size: points * factor).representation(using: .png, properties: [:])!
        try png.write(to: iconset.appendingPathComponent(name))
    }
}
// Next to the .icns: the 1024 PNG (README, previews), the 256 PNG Linux uses for the window
// and the desktop entry, and the .ico Windows embeds in moth.exe.
let base = output.deletingPathExtension()
try drawIcon(size: 1024).representation(using: .png, properties: [:])!
    .write(to: base.appendingPathExtension("png"))
try drawIcon(size: 256).representation(using: .png, properties: [:])!
    .write(to: URL(fileURLWithPath: base.path + "-256.png"))

/// An .ico file with PNG-compressed images, which Windows Vista and later read.
func ico(sizes: [Int]) -> Data {
    let images = sizes.map { drawIcon(size: $0).representation(using: .png, properties: [:])! }
    var data = Data()
    func append16(_ value: Int) { data.append(contentsOf: [UInt8(value & 0xFF), UInt8(value >> 8 & 0xFF)]) }
    func append32(_ value: Int) { append16(value & 0xFFFF); append16(value >> 16) }
    append16(0)
    append16(1)
    append16(sizes.count)
    var offset = 6 + 16 * sizes.count
    for (size, png) in zip(sizes, images) {
        // 256 is written as 0: the field is a single byte.
        data.append(contentsOf: [UInt8(size % 256), UInt8(size % 256), 0, 0])
        append16(1)
        append16(32)
        append32(png.count)
        append32(offset)
        offset += png.count
    }
    images.forEach { data.append($0) }
    return data
}
try ico(sizes: [16, 24, 32, 48, 64, 128, 256]).write(to: base.appendingPathExtension("ico"))

let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", output.path]
try iconutil.run()
iconutil.waitUntilExit()
try? FileManager.default.removeItem(at: iconset)
guard iconutil.terminationStatus == 0 else {
    FileHandle.standardError.write("iconutil failed\n".data(using: .utf8)!)
    exit(1)
}
print("Wrote \(output.path)")

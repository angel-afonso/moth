// Draws Moth's app icon and writes it as an .icns file.
//
// Usage: swift assets/branding/icon.swift assets/branding/moth.icns
//
// The icon is a dark rounded square, the macOS shape for app icons, with a moth in the
// accent color of the Graphite theme. Every size of the iconset is drawn from vectors, so
// the small ones stay sharp.

import AppKit

let accent = NSColor(srgbRed: 0x82 / 255, green: 0xAA / 255, blue: 0xFF / 255, alpha: 1)
let accentDeep = NSColor(srgbRed: 0x4F / 255, green: 0x6F / 255, blue: 0xC7 / 255, alpha: 1)
let backgroundTop = NSColor(srgbRed: 0x1F / 255, green: 0x23 / 255, blue: 0x2B / 255, alpha: 1)
let backgroundBottom = NSColor(srgbRed: 0x0C / 255, green: 0x0D / 255, blue: 0x10 / 255, alpha: 1)

/// Draws the icon on a 1024×1024 canvas, scaled to `size` pixels.
func drawIcon(size: Int) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let context = NSGraphicsContext.current!.cgContext
    let scale = CGFloat(size) / 1024
    context.scaleBy(x: scale, y: scale)

    // Apple's template: an 824-point square centered on the canvas, with a 185-point radius.
    let tile = NSRect(x: 100, y: 100, width: 824, height: 824)
    let shape = NSBezierPath(roundedRect: tile, xRadius: 185, yRadius: 185)
    NSGradient(starting: backgroundTop, ending: backgroundBottom)!.draw(in: shape, angle: -90)
    NSColor(white: 1, alpha: 0.08).setStroke()
    shape.lineWidth = 4
    shape.stroke()

    // A soft glow behind the moth, like the light it flies to.
    let glow = NSGradient(colors: [accent.withAlphaComponent(0.28), accent.withAlphaComponent(0)])!
    glow.draw(fromCenter: NSPoint(x: 512, y: 540), radius: 0, toCenter: NSPoint(x: 512, y: 540), radius: 330, options: [])

    context.saveGState()
    context.translateBy(x: 512, y: 500)

    // A moth at rest, seen from above: broad swept-back forewings, small hind wings under
    // them, a thick body and comb-like antennae (a butterfly's are thin with a club tip).
    for side in [-1.0, 1.0] as [CGFloat] {
        let hind = NSBezierPath()
        hind.move(to: NSPoint(x: 0, y: 20))
        hind.curve(to: NSPoint(x: side * 230, y: -70), controlPoint1: NSPoint(x: side * 100, y: 30), controlPoint2: NSPoint(x: side * 200, y: -10))
        hind.curve(to: NSPoint(x: side * 70, y: -210), controlPoint1: NSPoint(x: side * 260, y: -140), controlPoint2: NSPoint(x: side * 160, y: -215))
        hind.curve(to: NSPoint(x: 0, y: -120), controlPoint1: NSPoint(x: side * 30, y: -205), controlPoint2: NSPoint(x: side * 10, y: -170))
        hind.close()
        NSGradient(starting: accentDeep, ending: accentDeep.blended(withFraction: 0.35, of: backgroundBottom)!)!.draw(in: hind, angle: -90)

        let fore = NSBezierPath()
        fore.move(to: NSPoint(x: side * 10, y: 110))
        fore.curve(to: NSPoint(x: side * 345, y: 150), controlPoint1: NSPoint(x: side * 120, y: 190), controlPoint2: NSPoint(x: side * 270, y: 200))
        fore.curve(to: NSPoint(x: side * 250, y: -80), controlPoint1: NSPoint(x: side * 390, y: 110), controlPoint2: NSPoint(x: side * 330, y: -40))
        fore.curve(to: NSPoint(x: side * 10, y: -20), controlPoint1: NSPoint(x: side * 170, y: -110), controlPoint2: NSPoint(x: side * 60, y: -70))
        fore.close()
        NSGradient(starting: accent, ending: accentDeep)!.draw(in: fore, angle: -90)

        // Faint veins across the forewing.
        backgroundBottom.withAlphaComponent(0.28).setStroke()
        for (end, control) in [(NSPoint(x: side * 300, y: 120), NSPoint(x: side * 160, y: 130)),
                               (NSPoint(x: side * 260, y: 20), NSPoint(x: side * 150, y: 50)),
                               (NSPoint(x: side * 200, y: -60), NSPoint(x: side * 110, y: -20))] {
            let vein = NSBezierPath()
            vein.move(to: NSPoint(x: side * 30, y: 60))
            vein.curve(to: end, controlPoint1: control, controlPoint2: control)
            vein.lineWidth = 8
            vein.lineCapStyle = .round
            vein.stroke()
        }

        // Comb-like antenna: a curved stem with short barbs along it.
        let start = NSPoint(x: side * 12, y: 175)
        let tip = NSPoint(x: side * 150, y: 330)
        let control = NSPoint(x: side * 30, y: 290)
        accent.setStroke()
        let stem = NSBezierPath()
        stem.move(to: start)
        stem.curve(to: tip, controlPoint1: control, controlPoint2: control)
        stem.lineWidth = 10
        stem.lineCapStyle = .round
        stem.stroke()
        for step in 1...6 {
            let t = CGFloat(step) / 7
            let u = 1 - t
            // A point on the quadratic-like curve and the barb that leaves it.
            let x = u * u * start.x + 2 * u * t * control.x + t * t * tip.x
            let y = u * u * start.y + 2 * u * t * control.y + t * t * tip.y
            let length = 36 * (1 - t * 0.5)
            let barb = NSBezierPath()
            barb.move(to: NSPoint(x: x, y: y))
            barb.line(to: NSPoint(x: x + side * length, y: y - length * 0.35))
            barb.lineWidth = 7
            barb.lineCapStyle = .round
            barb.stroke()
        }
    }

    // Body: a fuzzy thorax and a tapering abdomen.
    let light = NSColor(srgbRed: 0xE3 / 255, green: 0xE6 / 255, blue: 0xEB / 255, alpha: 1)
    light.setFill()
    NSBezierPath(ovalIn: NSRect(x: -52, y: 40, width: 104, height: 150)).fill()
    let abdomen = NSBezierPath()
    abdomen.move(to: NSPoint(x: -44, y: 60))
    abdomen.curve(to: NSPoint(x: 0, y: -215), controlPoint1: NSPoint(x: -52, y: -80), controlPoint2: NSPoint(x: -36, y: -215))
    abdomen.curve(to: NSPoint(x: 44, y: 60), controlPoint1: NSPoint(x: 36, y: -215), controlPoint2: NSPoint(x: 52, y: -80))
    abdomen.close()
    abdomen.fill()
    backgroundBottom.withAlphaComponent(0.35).setStroke()
    for y in stride(from: -10, through: -170, by: -40) as StrideThrough<CGFloat> {
        let width = 40 - (abs(y) / 230) * 26
        let ring = NSBezierPath()
        ring.move(to: NSPoint(x: -width, y: y))
        ring.curve(to: NSPoint(x: width, y: y), controlPoint1: NSPoint(x: -width / 2, y: y - 10), controlPoint2: NSPoint(x: width / 2, y: y - 10))
        ring.lineWidth = 6
        ring.stroke()
    }

    context.restoreGState()
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
// The 1024 PNG next to the .icns, for the README and previews.
try drawIcon(size: 1024).representation(using: .png, properties: [:])!
    .write(to: output.deletingPathExtension().appendingPathExtension("png"))

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

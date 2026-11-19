// Renders the NagaButtons app icon: a white mouse symbol on a green tile.
// Usage: swift icon.swift out.png
import AppKit

let size = 1024.0
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                           bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let tile = NSBezierPath(roundedRect: NSRect(x: 100, y: 100, width: 824, height: 824), xRadius: 185, yRadius: 185)
NSGradient(starting: NSColor(srgbRed: 0.30, green: 0.86, blue: 0.38, alpha: 1),
           ending: NSColor(srgbRed: 0.04, green: 0.42, blue: 0.18, alpha: 1))!.draw(in: tile, angle: -90)
let config = NSImage.SymbolConfiguration(pointSize: 480, weight: .regular)
    .applying(NSImage.SymbolConfiguration(paletteColors: [.white]))
let mouse = NSImage(systemSymbolName: "computermouse.fill", accessibilityDescription: nil)!.withSymbolConfiguration(config)!
let s = mouse.size
mouse.draw(in: NSRect(x: (size - s.width) / 2, y: (size - s.height) / 2, width: s.width, height: s.height))
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))

import CoreGraphics
import Foundation
let owner = CommandLine.arguments[1]
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]
for w in list where (w[kCGWindowOwnerName as String] as? String) == owner {
  let b = w[kCGWindowBounds as String] as! [String: Any]
  let h = (b["Height"] as? Double) ?? 0
  if h > 200 { print(w[kCGWindowNumber as String]!, b["X"]!, b["Y"]!, b["Width"]!, h) }
}

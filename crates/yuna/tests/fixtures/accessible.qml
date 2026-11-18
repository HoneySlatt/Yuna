// The window crates/yuna/tests/mcp.rs reads through accessibility: a button away from the
// window's corner, whose name changes once it is pressed, and nothing that moves or blinks, so
// that once drawn it has no reason to draw again.
import QtQuick
import QtQuick.Controls.Basic

ApplicationWindow {
    width: 640
    height: 480
    visible: true
    title: "Yuna, accessible"

    Button {
        property bool pressedOnce: false

        x: 200
        y: 180
        width: 240
        height: 80
        text: pressedOnce ? "Pressed" : "Press me"
        onClicked: pressedOnce = true
    }
}

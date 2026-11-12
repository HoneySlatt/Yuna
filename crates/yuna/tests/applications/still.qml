// The window that crates/yuna/tests/applications.rs opens with Qt's `qml`: nothing in it moves,
// blinks or takes text, so that once drawn it has no reason to draw again.
import QtQuick
import QtQuick.Window

Window {
    width: 640
    height: 480
    visible: true
    title: "Yuna, still"
    color: "#203040"

    Text {
        anchors.centerIn: parent
        text: "Yuna"
        color: "white"
        font.pixelSize: 48
    }
}

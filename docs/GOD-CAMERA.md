# God camera (free camera)

The release client still has Funcom's debug free camera ("Godcam"). It needs a GM level with bit 4
(`lmoxide-server --data-dir <data> account gm <user> 6`, then log in again; accounts at the
`client` GM level and up get every client bit anyway). Protocol and RE notes: PROTOCOL.md
"Client god camera".

The keys below come from the 1.5 client (`GodCameraMode::SlotKeyboardKeyDown`, its on-screen
legend, `Data/Gui/Default/Hotkeys.xml`). God-cam keys are ignored while a text field (chat) has
focus.

## Turning it on and off

| Key | What it does |
|---|---|
| Ctrl+Alt+V | God camera on / off (back to the normal camera) |
| `/luac camera.god{pos={x, y, z}, rot={x, y, z, w}, fov=f}` in chat | God camera on, at that position and angle (see "Saving camera spots") |

## In god camera

| Key | What it does |
|---|---|
| W A S D | Move the camera |
| Q / E | Down / up |
| Right mouse button held + move the mouse | Look around (turn and tilt) |
| Page Up / Page Down | Faster / slower |
| Mouse wheel | Speed (the legend says "adjust speed") |
| Z | Lock the height on / off (the camera keeps its height above the ground) |
| X | Freeze the camera on / off |
| F | Put your character where the camera is (client side; the server sees it when you move) |
| R | Camera back to your character |
| Ctrl+1 … Ctrl+0 | Remember this camera spot as bookmark 1-0 |
| 1 … 0 | Go to that bookmark |
| Alt+F9 | Copy the camera as a `/luac camera.god{…}` line to the clipboard |

Bookmarks live only in the running client: they are gone after a restart. Use Alt+F9 and the
dev UI's Camera spots to keep them.

## Normal camera keys (everyone)

| Key | What it does |
|---|---|
| Z | Close-up camera on / off (lower, tilted view) |
| Shift+B | Reset the camera |
| Mouse wheel | Zoom |
| F9 | Your position into chat |
| Shift+F9 | Extended position into chat |
| Alt+F9 | Your position to the clipboard (outside god camera) |

## Saving camera spots

1. In god camera, frame the view and press **Alt+F9**. The clipboard now holds e.g.
   `/luac camera.god{pos={232.3, 140.0, 301.8}, rot={0.0, 0.0, 0.0, 1.0}, fov=60.0}`.
2. Paste it into the dev UI debugger: **Visuals → Camera spots → Save**, with a name. Spots are
   kept in that browser (export/import as JSON to move them).
3. To return: **Copy** on the spot, then in game open chat, Ctrl+V, Enter. `camera.god` turns the
   god camera on itself. A spot saved in another playfield also offers **Teleport to <playfield>**
   (`!tp`) first: the camera only shows the playfield you are in.

`/luac` runs Lua from chat and needs a GM level with bit 2 (accounts at the `client` GM level have
it). The server never learns where the camera is, so it can't save spots by itself.

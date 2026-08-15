# Part B: read the baseline, then define a class that exists ONLY in the
# primary's running world — no file, no image, nothing a fresh boot could find.
gui connect 7715
gui drain now
puts "WG7 classes-baseline [gui eval {WinShell browserClassCount}]"
gui doit {Worker uiRequest: #doit args: (Array with: 'Object subclass: GhostOfRestart [ ping [ ^42 ] ]') onReply: [ :r | nil ].}
gui drain now
gui doit {WinShell refreshBrowser.}

"""Write WG9's two test classes to the file-in path the host restarts from.

`world/115_winui_filein.mst` names the same filename and `win_gui`'s
`filein_scratch_path` derives the same directory — `GetTempPathW` there,
`std::env::temp_dir()` here, and Python's `gettempdir()` reads the same TMP/TEMP.
Nothing marshals the path; the coupling is one filename named in three places.

Written from Python rather than passed through the Tcl driver because a
Smalltalk method body is full of `[` and `]`, which inside a Tcl quoted string
are command substitution — the first attempt made Tcl try to run
`self runTest: #testAdds` as a command.

ONE PASSING CLASS AND ONE FAILING, which is the whole fixture: a suite that can
only report success is indistinguishable from a suite that always reports it.
"""

import io
import os
import tempfile

SRC = """TestCase subclass: WgNineOkTests [
    runAll [ self runTest: #testAdds do: [ self testAdds ] ]
    testAdds [ self assert: 1 + 1 = 2 description: 'one and one' ]
]
TestCase subclass: WgNineBadTests [
    runAll [
        self runTest: #testWrong do: [ self testWrong ].
        self runTest: #testRight do: [ self testRight ] ]
    testWrong [ self assert: 1 = 2 description: 'one is not two' ]
    testRight [ self assert: 2 = 2 description: 'two is two' ]
]
"""

path = os.path.join(tempfile.gettempdir(), "macvm-editor-filein.mst")
io.open(path, "w", encoding="utf-8", newline="\n").write(SRC)
print(path)

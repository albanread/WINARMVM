"bench-squeak.st — the Cog side of the WINARM race, ported from
 scripts/cog-bench.st (Pharo dialect) to Squeak 6.0 for the native
 win64ARMv8 Cog VM. The five micro benches are copied VERBATIM — same
 bodies, same iteration counts, same checksums — so the work is
 byte-identical to what scripts/cog-bench.mst runs on WINARM. Only the
 dialect seams differ, each noted:
   * Stdio stdout            -> results accumulated in a String stream and
                                written to cograce-results.txt (a GUI-
                                subsystem VM has no reliable stdout; a file
                                is the robust channel) AND echoed to
                                FileStream stdout for the console VM.
   * Smalltalk exitSuccess   -> Smalltalk snapshot: false andQuit: true.
   * Time microsecondClockValue -> probed at runtime; Squeak spells it
                                utcMicrosecondClock. Both are microseconds."!

"Marker as the FIRST chunk: proves the loader read the file at all —
 distinguishes 'never loaded' from 'died mid-file-in' (both have burned a
 run already: absolute paths get %-mangled by CodeLoader, and Squeak 6
 spells the class-definition keyword category:, not package:)."
(FileStream forceNewFileNamed: 'cograce-marker.txt') nextPutAll: 'filein-started'; close.!

Object subclass: #CogBench
	instanceVariableNames: ''
	classVariableNames: 'Log'
	poolDictionaries: ''
	category: 'CogBench'!

!CogBench class methodsFor: 'bench'!
usClock
	^(Time class canUnderstand: #microsecondClockValue)
		ifTrue: [ Time microsecondClockValue ]
		ifFalse: [ Time utcMicrosecondClock ]!
benchArith
	| s |
	s := 0.
	1 to: 1500000 do: [:i | s := s + (i * i) - (i * 3) ].
	^s!
fib: n
	n < 2 ifTrue: [ ^n ].
	^(self fib: n - 1) + (self fib: n - 2)!
benchFib
	^self fib: 32!
sieveOnce
	| flags count size |
	size := 8190.
	flags := Array new: size.
	1 to: size do: [:x | flags at: x put: true ].
	count := 0.
	1 to: size do: [:i |
		(flags at: i) ifTrue: [
			| prime k |
			prime := i + i + 1.
			k := i + prime.
			[ k <= size ] whileTrue: [ flags at: k put: false. k := k + prime ].
			count := count + 1 ] ].
	^count!
benchSieve
	| count |
	count := 0.
	4 timesRepeat: [ count := self sieveOnce ].
	^count!
benchDict
	| d sum |
	d := Dictionary new.
	1 to: 8000 do: [:i | d at: i put: i * i ].
	sum := 0.
	1 to: 8000 do: [:i | sum := sum + (d at: i) ].
	^sum!
benchAlloc
	| last |
	last := nil.
	1 to: 200000 do: [:i | last := Association key: i value: last ].
	^last key!
log: aString
	Log ifNil: [ Log := WriteStream on: String new ].
	Log nextPutAll: aString; nextPut: Character lf!
flushLog
	| f |
	f := FileStream forceNewFileNamed: 'cograce-results.txt'.
	[ f nextPutAll: Log contents ] ensure: [ f close ]!
check: name got: got want: want
	got = want ifFalse: [
		self log: name, ' WRONG RESULT ', got printString.
		self flushLog.
		Smalltalk snapshot: false andQuit: true ]!
run: name block: aBlock check: want
	"IDENTICAL protocol to scripts/cog-bench.mst and cog-bench.st: one cold
	 run, 30 discarded warm-ups to reach JIT steady state, then 41 single-
	 workload us samples; report median + min + MAD."
	| times cold t0 r sorted n med mn mad devs warm samples |
	warm := 30. samples := 41.
	t0 := self usClock. r := aBlock value.
	cold := self usClock - t0.
	self check: name got: r want: want.
	warm timesRepeat: [ r := aBlock value ].
	times := OrderedCollection new.
	samples timesRepeat: [
		t0 := self usClock.
		r := aBlock value.
		times add: self usClock - t0 ].
	self check: name got: r want: want.
	sorted := times asSortedCollection asArray.
	n := sorted size.
	med := sorted at: (n + 1) // 2.
	mn := sorted first.
	devs := (sorted collect: [:x | (x - med) abs]) asSortedCollection asArray.
	mad := devs at: (devs size + 1) // 2.
	self log: name, ' cold_us=', cold printString, ' warm_us=', med printString,
		' min_us=', mn printString, ' mad_us=', mad printString, ' n=', samples printString!
runAll
	"Every step inside one Error trap: a headless dialect surprise must land
	 in the results file, never in a debugger window nobody can see."
	[ self log: 'vm=', Smalltalk vmVersion.
	  self run: 'arith    ' block: [ self benchArith ] check: 1124997749998000000.
	  self run: 'fib      ' block: [ self benchFib ] check: 2178309.
	  self run: 'sieve    ' block: [ self benchSieve ] check: 1899.
	  self run: 'dict     ' block: [ self benchDict ] check: 170698668000.
	  self run: 'alloc    ' block: [ self benchAlloc ] check: 200000. ]
		on: Error
		do: [:e | self log: 'ERROR ', e class name, ': ', e messageText printString ].
	self flushLog.! !

CogBench runAll.!
Smalltalk snapshot: false andQuit: true.!

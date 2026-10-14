Type "( get $url $sum models/model.bin & get $url $sum models/model.bin 2> /dev/null; wait )"
Enter
Wait@20s /^\$\s*$/
Sleep 800ms
Type "ls models && sha256sum models/model.bin"
Enter
Sleep 3s

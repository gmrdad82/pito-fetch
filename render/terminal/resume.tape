Type "get http://127.0.0.1:8471/model.bin $sum models/model.bin"
Enter
Sleep 2200ms
Ctrl+C
Sleep 600ms
Type "ls models"
Enter
Sleep 1500ms
Type "get http://127.0.0.1:8471/model.bin $sum models/model.bin"
Enter
Wait@20s /^\$\s*$/
Sleep 1500ms
Type "get http://127.0.0.1:8471/model.bin $bad models/copy.bin"
Enter
Wait@20s /^\$\s*$/
Sleep 800ms
Type "ls models"
Enter
Sleep 3s

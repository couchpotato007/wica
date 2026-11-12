# Demo GIF for the README. Render with: vhs cassette.tape
Output demo.gif

Require wica

Set Shell fish
Set FontSize 18
Set Width 1200
Set Height 720
Set Padding 24
Set Margin 24
Set MarginFill "#6B50FF"
Set BorderRadius 12
Set WindowBar Colorful
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms
Set CursorBlink false

# Setup that shouldn't be in the GIF: a plain prompt, no greeting, no grey
# autosuggestions, and one warm-up run so the index and package lookups are
# cached. The leading space keeps these commands out of fish's history.
Hide
Type " set -g fish_autosuggestion_enabled 0; set -g fish_greeting; function fish_prompt; set_color brmagenta; echo -n '❯ '; set_color normal; end"
Enter
Type " for q in 'render markdown in terminal' 'search text in files'; wica (string split ' ' $q) -n 3 >/dev/null 2>&1; end; wica name deer >/dev/null 2>&1; clear"
Enter
Sleep 20s
Show

Sleep 500ms

# 1. Describe what the tool does
Type "wica render markdown in terminal -n 3"
Sleep 400ms
Enter
Sleep 4s

Type "clear"
Enter
Sleep 300ms

# 2. Another description, different wording
Type "wica search text in files -n 3"
Sleep 400ms
Enter
Sleep 4s

Type "clear"
Enter
Sleep 300ms

# 3. Half remember the name
Type "wica name deer"
Sleep 400ms
Enter
Sleep 4s

Type "clear"
Enter
Sleep 300ms

# 4. All examples for a tool
Type "wica tldr glow"
Sleep 400ms
Enter
Sleep 5s

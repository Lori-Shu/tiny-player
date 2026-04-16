# Basic
A media player, aims to be light and simple;
core feature based on project
[ffmpeg](https://github.com/FFmpeg/FFmpeg)
[ffmpeg-the-third](https://github.com/shssoichiro/ffmpeg-the-third)
[egui](https://github.com/emilk/egui)
# Why should you use tiny-player
1. tiny-player is small
2. tiny-player is fast, with pure rust language, modern ui 
   framework [egui](https://github.com/emilk/egui) and maybe the 
   fastest decoder supplied by [ffmpeg](https://github.com/FFmpeg/FFmpeg) 
   and even more, rendered by 
   [wgpu](https://github.com/gfx-rs/wgpu) with vulkan backend
3. tiny-player is opensource, you can modify code and build your 
   own app under LICENSE
# Usage
1. currently only support windows
2. run the tiny-player-setup.exe
3. run the tiny-player.exe on desktop
4. click the file button
5. select a media file, normally .mp4 or .mkv
6. click open 
7. click the play button
8. control the progress by the control widges
# Tips
1. if you use a laptop and player runs at a very low frame 
rate, go to settings->system->power&battery, set power mode of "on battery" to 
"best performance" or "balanced" so that windows allows cpu to run at a higher clock speed.
# Screenshot
![app screenshot](./project_show_img.png)

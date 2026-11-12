# Tunnelrat release shrinking (R8). The app uses no reflection of its own;
# libraries (OkHttp, ZXing, CameraX, AndroidX) ship their own consumer rules.
# Keep line numbers so crash reports from release builds stay readable.
-keepattributes SourceFile,LineNumberTable
-renamesourcefileattribute SourceFile

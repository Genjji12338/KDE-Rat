package dev.tunnelrat.ui.scan

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import android.util.Size
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.compose.CameraXViewfinder
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.core.SurfaceRequest
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.lifecycle.awaitInstance
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.FlashlightOff
import androidx.compose.material.icons.rounded.FlashlightOn
import androidx.compose.material.icons.rounded.NoPhotography
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size as GeoSize
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import dev.tunnelrat.ui.Tura
import dev.tunnelrat.ui.blockTouchesBelow
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.delay
import java.util.concurrent.Executors

/** Fraction of the shorter screen side used by the square viewfinder. */
private const val FRAME_FRACTION = 0.70f

/**
 * Full-screen QR scanner for pairing links (`tunnelrat://pair?...`).
 * Calls [onResult] once with the raw link; other QR codes are ignored with a hint.
 */
@Composable
fun ScannerScreen(onResult: (String) -> Unit, onClose: () -> Unit) {
    val context = LocalContext.current
    var granted by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    var denied by remember { mutableStateOf(false) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        granted = ok
        denied = !ok
    }
    LaunchedEffect(Unit) { if (!granted) ask.launch(Manifest.permission.CAMERA) }
    BackHandler(onBack = onClose)

    Box(Modifier.fillMaxSize().background(Color.Black).blockTouchesBelow()) {
        if (granted) {
            CameraScanner(onResult = onResult)
        } else if (denied) {
            NoCamera(
                onOpenSettings = {
                    context.startActivity(
                        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                    )
                },
                onRetry = { ask.launch(Manifest.permission.CAMERA) },
            )
        }

        // Close button sits above everything, including the permission screen.
        RoundButton(Icons.Rounded.Close, "Close scanner", Modifier.align(Alignment.TopStart).safeDrawingPadding().padding(16.dp), onClick = onClose)
    }
}

@Composable
private fun CameraScanner(onResult: (String) -> Unit) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val haptics = LocalHapticFeedback.current
    val currentOnResult by rememberUpdatedState(onResult)

    var surfaceRequest by remember { mutableStateOf<SurfaceRequest?>(null) }
    var camera by remember { mutableStateOf<Camera?>(null) }
    var torch by remember { mutableStateOf(false) }
    var found by remember { mutableStateOf<String?>(null) }
    var hint by remember { mutableStateOf<String?>(null) }
    var hintAt by remember { mutableStateOf(0L) }

    val executor = remember { Executors.newSingleThreadExecutor() }
    DisposableEffect(Unit) { onDispose { executor.shutdown() } }

    val main = remember { ContextCompat.getMainExecutor(context) }
    val analyzer = remember {
        QrAnalyzer { text ->
            main.execute {
                if (found != null) return@execute
                if (text.startsWith("tunnelrat://pair?")) {
                    found = text
                } else if (System.currentTimeMillis() - hintAt > 2500) {
                    hintAt = System.currentTimeMillis()
                    hint = "That's not a Tunnelrat pairing code"
                }
            }
        }
    }

    LaunchedEffect(lifecycleOwner) {
        val provider = ProcessCameraProvider.awaitInstance(context)
        val preview = Preview.Builder().build().apply { setSurfaceProvider { req -> surfaceRequest = req } }
        val analysis = ImageAnalysis.Builder()
            .setResolutionSelector(
                ResolutionSelector.Builder()
                    .setResolutionStrategy(ResolutionStrategy(Size(1280, 720), ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER))
                    .build(),
            )
            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
            .build()
            .apply { setAnalyzer(executor, analyzer) }
        provider.unbindAll()
        camera = provider.bindToLifecycle(lifecycleOwner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
        try {
            awaitCancellation()
        } finally {
            provider.unbindAll()
        }
    }

    LaunchedEffect(torch, camera) { camera?.cameraControl?.enableTorch(torch) }

    LaunchedEffect(found) {
        val link = found ?: return@LaunchedEffect
        analyzer.pause(true)
        haptics.performHapticFeedback(HapticFeedbackType.Confirm)
        delay(450) // let the green "locked" frame register
        currentOnResult(link)
    }

    LaunchedEffect(hint) {
        if (hint != null) {
            delay(2200)
            hint = null
        }
    }

    surfaceRequest?.let { CameraXViewfinder(surfaceRequest = it, modifier = Modifier.fillMaxSize()) }
    ViewfinderOverlay(locked = found != null)

    Column(Modifier.fillMaxSize().safeDrawingPadding().padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            if (camera?.cameraInfo?.hasFlashUnit() == true) {
                RoundButton(
                    if (torch) Icons.Rounded.FlashlightOn else Icons.Rounded.FlashlightOff,
                    if (torch) "Turn torch off" else "Turn torch on",
                    active = torch,
                ) { torch = !torch }
            }
        }
        Spacer(Modifier.weight(1f))
        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
            AnimatedVisibility(hint != null, enter = fadeIn(), exit = fadeOut()) {
                Text(
                    hint.orEmpty(),
                    color = Tura.Text,
                    fontSize = 13.sp,
                    modifier = Modifier.clip(CircleShape).background(Tura.Danger.copy(alpha = 0.85f)).padding(horizontal = 14.dp, vertical = 6.dp),
                )
            }
            Text(
                if (found != null) "Got it" else "Scan the pairing code",
                color = Tura.Text,
                fontSize = 22.sp,
                fontWeight = FontWeight.Bold, fontFamily = Tura.Display,
            )
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Run ", color = Tura.Muted, fontSize = 14.sp)
                Text(
                    "tura pair",
                    color = Tura.Amber,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Bold,
                    fontFamily = FontFamily.Monospace,
                    modifier = Modifier.clip(RoundedCornerShape(8.dp)).background(Tura.Bg.copy(alpha = 0.8f)).padding(horizontal = 8.dp, vertical = 3.dp),
                )
                Text(" on your PC", color = Tura.Muted, fontSize = 14.sp)
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}

/** Dimmed surround, a clear rounded square, amber corner brackets and a sweeping scan line. */
@Composable
private fun ViewfinderOverlay(locked: Boolean) {
    val bracket by animateColorAsState(if (locked) Tura.Ok else Tura.Amber, label = "bracket")
    val sweep by rememberInfiniteTransition(label = "sweep").animateFloat(
        initialValue = 0.08f,
        targetValue = 0.92f,
        animationSpec = infiniteRepeatable(tween(1600, easing = LinearEasing), RepeatMode.Reverse),
        label = "sweep",
    )

    Canvas(Modifier.fillMaxSize().graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen)) {
        val side = size.minDimension * FRAME_FRACTION
        val frame = Rect(Offset((size.width - side) / 2f, (size.height - side) / 2f - size.height * 0.06f), GeoSize(side, side))
        val radius = 32.dp.toPx()

        drawRect(Color.Black.copy(alpha = 0.62f))
        drawRoundRect(Color.Transparent, frame.topLeft, frame.size, CornerRadius(radius), blendMode = BlendMode.Clear)

        // Corner brackets: an arc plus two short legs per corner.
        val leg = side * 0.16f
        val stroke = Stroke(width = 5.dp.toPx(), cap = StrokeCap.Round)
        fun corner(cx: Float, cy: Float, sx: Float, sy: Float) {
            // (cx, cy) = corner point; (sx, sy) = direction into the frame.
            val arcRect = Rect(
                Offset(if (sx > 0) cx else cx - 2 * radius, if (sy > 0) cy else cy - 2 * radius),
                GeoSize(2 * radius, 2 * radius),
            )
            // Start where the vertical leg ends: 180° on left corners, 0° on
            // right ones; sweep toward the horizontal leg.
            val start = if (sx > 0) 180f else 0f
            val sweepDeg = 90f * sx * sy
            val p = Path().apply {
                moveTo(cx, cy + sy * (radius + leg))
                lineTo(cx, cy + sy * radius)
                arcTo(arcRect, start, sweepDeg, false)
                lineTo(cx + sx * (radius + leg), cy)
            }
            drawPath(p, bracket, style = stroke)
        }
        corner(frame.left, frame.top, 1f, 1f)
        corner(frame.right, frame.top, -1f, 1f)
        corner(frame.right, frame.bottom, -1f, -1f)
        corner(frame.left, frame.bottom, 1f, -1f)

        if (!locked) {
            val y = frame.top + frame.height * sweep
            val inset = side * 0.08f
            drawLine(
                brush = Brush.horizontalGradient(
                    listOf(Color.Transparent, Tura.Amber.copy(alpha = 0.9f), Color.Transparent),
                    startX = frame.left + inset,
                    endX = frame.right - inset,
                ),
                start = Offset(frame.left + inset, y),
                end = Offset(frame.right - inset, y),
                strokeWidth = 2.dp.toPx(),
                cap = StrokeCap.Round,
            )
        }
    }
}

@Composable
private fun NoCamera(onOpenSettings: () -> Unit, onRetry: () -> Unit) {
    Column(
        Modifier.fillMaxSize().background(Tura.Bg).safeDrawingPadding().padding(32.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(Icons.Rounded.NoPhotography, contentDescription = null, tint = Tura.Muted, modifier = Modifier.size(64.dp))
        Text("Camera access needed", color = Tura.Text, fontSize = 20.sp, fontWeight = FontWeight.Bold, fontFamily = Tura.Display)
        Text(
            "Tunnelrat only uses the camera to read the pairing QR code. Nothing is recorded or sent anywhere.",
            color = Tura.Muted,
            fontSize = 14.sp,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(4.dp))
        Pill("Allow camera", filled = true, onClick = onRetry)
        Pill("Open app settings", filled = false, onClick = onOpenSettings)
    }
}

@Composable
private fun Pill(label: String, filled: Boolean, onClick: () -> Unit) {
    Box(
        Modifier.clip(CircleShape)
            .background(if (filled) Tura.Amber else Color.Transparent)
            .border(1.dp, if (filled) Color.Transparent else Tura.Line, CircleShape)
            .clickable(onClick = onClick)
            .padding(horizontal = 22.dp, vertical = 12.dp),
    ) { Text(label, color = if (filled) Tura.Bg else Tura.Text, fontWeight = FontWeight.Bold) }
}

@Composable
private fun RoundButton(icon: ImageVector, description: String, modifier: Modifier = Modifier, active: Boolean = false, onClick: () -> Unit) {
    Box(
        modifier.size(48.dp).clip(CircleShape)
            .background(if (active) Tura.Amber else Color.Black.copy(alpha = 0.45f))
            .border(1.dp, Color.White.copy(alpha = 0.15f), CircleShape)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Icon(icon, contentDescription = description, tint = if (active) Tura.Bg else Color.White) }
}

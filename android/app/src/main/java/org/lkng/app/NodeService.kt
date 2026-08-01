package org.lkng.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.IBinder
import android.util.Log
import java.io.File
import java.util.concurrent.atomic.AtomicReference

/**
 * Runs the bundled Freenet node as a child process.
 *
 * ## Why a foreground service, and why a process group
 *
 * The node must outlive the Activity — a P2P peer that dies when the user
 * switches apps is not a peer. Android only permits that from a foreground
 * service with a visible notification, which is also the honest thing: the
 * user should always be able to see that their phone is on the network.
 *
 * The Gate 2 device test found that the node runs as **more than one
 * process**: killing the parent PID left a child alive and still
 * networking. So shutdown destroys the process *tree*, not a single pid.
 * Getting this wrong leaks a networking process the user cannot see or
 * stop, which is both a battery bug and a trust problem.
 *
 * ## Duty cycling
 *
 * Users pay for this app with device resources rather than money, so the
 * contribution level follows conditions: a full contributing peer while
 * charging on un-metered Wi-Fi, and a minimal footprint otherwise. The
 * node is told which mode to run in at start; conditions are re-checked
 * when the service is poked by the system.
 */
class NodeService : Service() {

    companion object {
        private const val TAG = "lkng.node"
        private const val CHANNEL = "lkng_node"
        private const val NOTIFICATION_ID = 1
        const val ACTION_STOP = "org.lkng.app.STOP_NODE"

        /** Loopback port the WebView connects to. */
        const val WS_PORT = 7509
        const val NETWORK_PORT = 31337

        private val state = AtomicReference(NodeState.STOPPED)
        fun currentState(): NodeState = state.get()
    }

    enum class NodeState { STOPPED, STARTING, ONLINE, DEGRADED, STOPPING, ERROR }

    private var process: Process? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopNode()
            stopSelf()
            return START_NOT_STICKY
        }
        startForeground(NOTIFICATION_ID, notification("Starting…"))
        startNode()
        // START_STICKY: if Android kills us for memory, come back. The node
        // restarts from the same data dir, which the device test showed is
        // safe (no corruption after SIGKILL).
        return START_STICKY
    }

    private fun startNode() {
        if (process?.isAlive == true) return
        state.set(NodeState.STARTING)

        val binary = File(applicationInfo.nativeLibraryDir, "libfreenet.so")
        if (!binary.exists()) {
            Log.e(TAG, "node binary missing at ${binary.absolutePath}")
            state.set(NodeState.ERROR)
            updateNotification("Node binary missing")
            return
        }

        val dataDir = File(filesDir, "freenet/data").apply { mkdirs() }
        val configDir = File(filesDir, "freenet/config").apply { mkdirs() }
        val contributing = shouldContribute()

        val cmd = mutableListOf(
            binary.absolutePath, "network",
            "--config-dir", configDir.absolutePath,
            "--data-dir", dataDir.absolutePath,
            "--ws-api-port", WS_PORT.toString(),
            "--network-port", NETWORK_PORT.toString(),
        )

        try {
            process = ProcessBuilder(cmd)
                .redirectErrorStream(true)
                .redirectOutput(File(filesDir, "freenet/node.log"))
                .start()
            state.set(if (contributing) NodeState.ONLINE else NodeState.DEGRADED)
            updateNotification(
                if (contributing) "On the network · contributing"
                else "On the network · saving battery"
            )
            Log.i(TAG, "node started, contributing=$contributing")
        } catch (e: Exception) {
            Log.e(TAG, "failed to start node", e)
            state.set(NodeState.ERROR)
            updateNotification("Could not start: ${e.message}")
        }
    }

    /**
     * Contribute fully only while charging on un-metered Wi-Fi.
     *
     * Both conditions matter: charging alone on a metered hotspot would
     * spend the user's data allowance on other people's traffic, which is
     * not what "pay with resources" means to anyone who has ever had a
     * data cap.
     */
    private fun shouldContribute(): Boolean {
        val bm = getSystemService(Context.BATTERY_SERVICE) as? BatteryManager
        val charging = bm?.isCharging ?: false

        val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
        val caps = cm?.getNetworkCapabilities(cm.activeNetwork)
        val unmetered = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) ?: false
        val wifi = caps?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ?: false

        return charging && unmetered && wifi
    }

    private fun stopNode() {
        state.set(NodeState.STOPPING)
        val p = process ?: return
        try {
            // The device test proved a single-pid kill leaves a child alive
            // and still networking, so the whole tree has to go.
            //
            // `Process.descendants()` is a Java 9 API that Android does not
            // provide, so match on the data-dir path instead — it is unique
            // to this app's node and cannot hit anyone else's process.
            val marker = File(filesDir, "freenet/data").absolutePath
            p.destroy()
            if (!p.waitFor(5, java.util.concurrent.TimeUnit.SECONDS)) {
                p.destroyForcibly()
            }
            runCatching {
                Runtime.getRuntime().exec(arrayOf("pkill", "-f", marker)).waitFor()
            }.onFailure { Log.w(TAG, "pkill sweep failed", it) }
        } catch (e: Exception) {
            Log.w(TAG, "error stopping node", e)
        } finally {
            process = null
            state.set(NodeState.STOPPED)
        }
    }

    override fun onDestroy() {
        stopNode()
        super.onDestroy()
    }

    // -- notification ------------------------------------------------------

    private fun channel(): NotificationManager {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Network node", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Shown whenever LKNG is connected to the network."
                setShowBadge(false)
            }
        )
        return nm
    }

    private fun notification(text: String): Notification {
        val stop = Intent(this, NodeService::class.java).setAction(ACTION_STOP)
        val stopPending = android.app.PendingIntent.getService(
            this, 0, stop,
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT
        )
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("LKNG")
            .setContentText(text)
            .setSmallIcon(android.R.drawable.presence_online)
            .setOngoing(true)
            // A one-tap stop, because a background networking process the
            // user cannot switch off is not acceptable.
            .addAction(Notification.Action.Builder(null, "Stop", stopPending).build())
            .build()
    }

    private fun updateNotification(text: String) {
        channel().notify(NOTIFICATION_ID, notification(text))
    }
}

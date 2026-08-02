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
import java.net.InetSocketAddress
import java.net.Socket
import kotlin.concurrent.thread
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

        // Make the duty cycle real.
        //
        // This block did not exist until 2026-08-01. `shouldContribute()`
        // was computed and used only to choose the notification text, so a
        // phone showing "saving battery" was doing exactly what a
        // contributing one did. Measured while dozing: ~41% of one core,
        // sustained, plugged in or not.
        //
        // The node accepts real limits, so use them. Fewer ring connections
        // is the lever that matters: each one is a peer whose traffic this
        // phone relays, and relaying is most of the cost.
        if (!contributing) {
            cmd += listOf(
                "--max-number-of-connections", LEAN_CONNECTIONS.toString(),
                "--total-bandwidth-limit", LEAN_BANDWIDTH_BPS.toString(),
            )
        }

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
            startHealthCheck()
            Log.i(
                TAG,
                "node started, contributing=$contributing" +
                    if (contributing) "" else
                        " (capped at $LEAN_CONNECTIONS connections, " +
                        "$LEAN_BANDWIDTH_BPS B/s)"
            )
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
    /**
     * Ring connections while saving battery.
     *
     * Not zero, and not one. A node with too few connections cannot route,
     * which would make "saving battery" mean "silently left the network" —
     * the user would stop receiving messages and nothing would say so. Five
     * keeps it a participating peer at roughly a quarter of the relay load.
     */
    /** How often the health check probes the client port. */
    private val HEALTH_INTERVAL_MS = 2 * 60 * 1000L

    @Volatile private var healthThread: Thread? = null
    @Volatile private var stopping = false

    private val LEAN_CONNECTIONS = 5

    /** 200 KB/s total while saving battery, against a 3 MB/s default. */
    private val LEAN_BANDWIDTH_BPS = 200_000

    private fun shouldContribute(): Boolean {
        val bm = getSystemService(Context.BATTERY_SERVICE) as? BatteryManager
        val charging = bm?.isCharging ?: false

        val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
        val caps = cm?.getNetworkCapabilities(cm.activeNetwork)
        val unmetered = caps?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) ?: false
        val wifi = caps?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ?: false

        return charging && unmetered && wifi
    }

    /**
     * Restart the node if it stops accepting client connections.
     *
     * ## Why this is necessary rather than paranoid
     *
     * The node accumulates established connections on its client API whose
     * peers no longer exist — 7 of them within 34 minutes of a clean restart,
     * with nothing connected. Earlier in development the same accumulation
     * reached `LISTEN 129 128`: the accept backlog was full and the node
     * refused every new client.
     *
     * That failure is quiet and cruel on a phone. The process is alive, the
     * foreground service says "On the network", and the app cannot reach its
     * own node — so the user sees an empty grid and no messages, with
     * everything insisting it is fine. A liveness check that only asks
     * "is the process running?" would report healthy throughout.
     *
     * So the check is what the app actually needs: **can a socket be opened
     * to the client port?** If not, twice in a row, the node is restarted.
     *
     * ## Why twice, and why the interval is minutes
     *
     * A single failure can be a busy moment during startup or a network
     * transition. Restarting on one is how a health check becomes a restart
     * loop that is worse than the fault it was added for. Two minutes apart
     * is far below the timescale on which the backlog fills (hours) and far
     * above any transient.
     */
    private fun startHealthCheck() {
        if (healthThread != null) return
        healthThread = thread(isDaemon = true, name = "lkng-health") {
            var consecutiveFailures = 0
            while (!stopping) {
                Thread.sleep(HEALTH_INTERVAL_MS)
                if (stopping) break
                if (state.get() != NodeState.ONLINE && state.get() != NodeState.DEGRADED) {
                    continue
                }
                val reachable = try {
                    Socket().use { sock ->
                        sock.connect(InetSocketAddress("127.0.0.1", WS_PORT), 3000)
                        true
                    }
                } catch (e: Exception) {
                    false
                }

                if (reachable) {
                    consecutiveFailures = 0
                } else {
                    consecutiveFailures++
                    Log.w(TAG, "node not accepting clients ($consecutiveFailures)")
                    if (consecutiveFailures >= 2) {
                        Log.w(TAG, "restarting node: client port unreachable twice")
                        consecutiveFailures = 0
                        stopNode()
                        Thread.sleep(2000)
                        startNode()
                    }
                }
            }
        }
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
        // Let the health thread exit before the node goes down, or it sees
        // the port close and "helpfully" restarts a service the user just
        // stopped -- a node that will not stay stopped is worse than one
        // that will not stay running.
        stopping = true
        healthThread?.interrupt()
        healthThread = null
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

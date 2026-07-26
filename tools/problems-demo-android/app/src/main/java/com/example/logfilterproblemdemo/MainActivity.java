package com.example.logfilterproblemdemo;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Color;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.os.SystemClock;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.util.Log;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.List;

public final class MainActivity extends Activity {
    private static final String TAG = "LogFilterDemo";
    private static final long CONFIRM_TIMEOUT_MS = 5_000;
    private static final long ACTION_DELAY_MS = 600;

    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private final ArmingGate armingGate = new ArmingGate(CONFIRM_TIMEOUT_MS);
    private TextView statusView;

    private final Runnable disarmRunnable =
            () -> {
                armingGate.clear();
                statusView.setText(R.string.status_cancelled);
            };

    private enum DemoAction {
        ERROR_ONLY(R.string.action_error_only, false),
        JAVA_CRASH(R.string.action_java_crash, true),
        JAVA_OOM(R.string.action_java_oom, true),
        ANR(R.string.action_anr, true),
        NATIVE_CRASH(R.string.action_native_crash, true),
        PROCESS_EXIT(R.string.action_process_exit, true);

        final int labelRes;
        final boolean requiresConfirmation;

        DemoAction(int labelRes, boolean requiresConfirmation) {
            this.labelRes = labelRes;
            this.requiresConfirmation = requiresConfirmation;
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        setContentView(createContentView());
    }

    @Override
    protected void onDestroy() {
        mainHandler.removeCallbacks(disarmRunnable);
        super.onDestroy();
    }

    private View createContentView() {
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setGravity(Gravity.CENTER_HORIZONTAL);
        root.setPadding(dp(52), dp(30), dp(52), dp(30));
        root.setBackgroundColor(getColor(R.color.screen_background));

        TextView title = new TextView(this);
        title.setText(R.string.screen_title);
        title.setTextColor(getColor(R.color.text_primary));
        title.setTextSize(30);
        title.setGravity(Gravity.CENTER);
        root.addView(
                title,
                new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        TextView subtitle = new TextView(this);
        subtitle.setText(R.string.screen_subtitle);
        subtitle.setTextColor(getColor(R.color.text_secondary));
        subtitle.setTextSize(18);
        subtitle.setGravity(Gravity.CENTER);
        LinearLayout.LayoutParams subtitleParams =
                new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        subtitleParams.topMargin = dp(6);
        root.addView(subtitle, subtitleParams);

        statusView = new TextView(this);
        statusView.setText(R.string.status_ready);
        statusView.setTextColor(getColor(R.color.status_warning));
        statusView.setTextSize(18);
        statusView.setGravity(Gravity.CENTER);
        statusView.setPadding(dp(20), dp(12), dp(20), dp(12));
        statusView.setBackgroundColor(getColor(R.color.panel_background));
        LinearLayout.LayoutParams statusParams =
                new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        statusParams.topMargin = dp(18);
        statusParams.bottomMargin = dp(14);
        root.addView(statusView, statusParams);

        LinearLayout buttonColumn = new LinearLayout(this);
        buttonColumn.setOrientation(LinearLayout.VERTICAL);
        buttonColumn.setGravity(Gravity.CENTER_HORIZONTAL);

        Button firstButton = null;
        for (DemoAction action : DemoAction.values()) {
            Button button = createActionButton(action);
            if (firstButton == null) {
                firstButton = button;
            }
            buttonColumn.addView(button, actionButtonLayoutParams());
        }

        ScrollView scrollView = new ScrollView(this);
        scrollView.setFillViewport(true);
        scrollView.setClipToPadding(false);
        scrollView.addView(
                buttonColumn,
                new ScrollView.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        root.addView(
                scrollView,
                new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, 0, 1));

        if (firstButton != null) {
            firstButton.requestFocus();
        }
        return root;
    }

    private Button createActionButton(DemoAction action) {
        Button button = new Button(this);
        button.setText(action.labelRes);
        button.setTextColor(Color.WHITE);
        button.setTextSize(21);
        button.setAllCaps(false);
        button.setGravity(Gravity.CENTER);
        button.setFocusable(true);
        button.setFocusableInTouchMode(true);
        button.setMinHeight(dp(60));
        button.setPadding(dp(18), dp(10), dp(18), dp(10));
        button.setBackgroundResource(R.drawable.action_button_background);
        button.setOnClickListener(view -> onActionPressed(action, button));
        return button;
    }

    private LinearLayout.LayoutParams actionButtonLayoutParams() {
        LinearLayout.LayoutParams params =
                new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        params.setMargins(0, dp(5), 0, dp(5));
        return params;
    }

    private void onActionPressed(DemoAction action, Button button) {
        if (!action.requiresConfirmation) {
            executeAction(action, button);
            return;
        }

        ArmingGate.Decision decision =
                armingGate.accept(action.name(), SystemClock.elapsedRealtime());
        if (decision == ArmingGate.Decision.ARMED) {
            mainHandler.removeCallbacks(disarmRunnable);
            statusView.setText(
                    getString(R.string.status_armed, getString(action.labelRes)));
            mainHandler.postDelayed(disarmRunnable, CONFIRM_TIMEOUT_MS);
            return;
        }

        mainHandler.removeCallbacks(disarmRunnable);
        statusView.setText(
                getString(R.string.status_running, getString(action.labelRes)));
        button.setEnabled(false);
        mainHandler.postDelayed(() -> executeAction(action, button), ACTION_DELAY_MS);
    }

    private void executeAction(DemoAction action, Button sourceButton) {
        switch (action) {
            case ERROR_ONLY:
                emitNegativeControlErrors();
                statusView.setText(R.string.status_error_logged);
                return;
            case JAVA_CRASH:
                throw new IllegalStateException(
                        "LogFilter Problems demo: deterministic Java crash");
            case JAVA_OOM:
                startJavaOom();
                return;
            case ANR:
                triggerBroadcastAnr(sourceButton);
                return;
            case NATIVE_CRASH:
                sendNativeCrashSignal();
                return;
            case PROCESS_EXIT:
                exitProcess();
                return;
        }
    }

    private void emitNegativeControlErrors() {
        for (int index = 1; index <= 5; index++) {
            Log.e(
                    TAG,
                    "Negative control "
                            + index
                            + "/5: simulated component failure; app remains healthy");
        }
    }

    private void startJavaOom() {
        Thread oomThread =
                new Thread(
                        () -> {
                            List<byte[]> allocations = new ArrayList<>();
                            while (true) {
                                allocations.add(new byte[4 * 1024 * 1024]);
                            }
                        },
                        "LogFilter-OOM");
        oomThread.start();
    }

    private void triggerBroadcastAnr(Button sourceButton) {
        Intent intent = new Intent(this, AnrBroadcastReceiver.class);
        intent.setAction(AnrBroadcastReceiver.ACTION_TRIGGER_ANR);
        intent.addFlags(Intent.FLAG_RECEIVER_FOREGROUND);

        mainHandler.postDelayed(
                () -> {
                    sourceButton.setEnabled(true);
                    statusView.setText(R.string.status_anr_recovered);
                },
                31_000);
        sendBroadcast(intent);
    }

    private void sendNativeCrashSignal() {
        Log.i(TAG, "Sending SIGSEGV to the demo process");
        try {
            Os.kill(Process.myPid(), OsConstants.SIGSEGV);
        } catch (ErrnoException error) {
            throw new IllegalStateException("Unable to send SIGSEGV", error);
        }
    }

    private void exitProcess() {
        Log.i(TAG, "Ending the demo process; relaunch to observe a new process start");
        Process.killProcess(Process.myPid());
        System.exit(10);
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}

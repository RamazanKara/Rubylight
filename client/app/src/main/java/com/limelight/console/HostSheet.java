package com.limelight.console;

import android.content.Context;
import android.content.res.Configuration;
import android.content.res.Resources;
import android.graphics.Rect;
import android.util.AttributeSet;
import android.view.FocusFinder;
import android.view.Gravity;
import android.view.LayoutInflater;
import android.view.MotionEvent;
import android.view.VelocityTracker;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.activity.ComponentActivity;
import androidx.activity.OnBackPressedCallback;
import androidx.core.content.res.ResourcesCompat;
import androidx.dynamicanimation.animation.DynamicAnimation;
import androidx.dynamicanimation.animation.SpringAnimation;
import androidx.dynamicanimation.animation.SpringForce;

import com.limelight.R;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.GlassForm;
import com.limelight.ui.Motion;

import java.util.List;
import java.util.function.Consumer;

/**
 * The PCs behind Home's capsule, in glass over the page: a status dot, what the PC is doing, and a mark on the one Home shows.
 * It lives in the activity's own glass host, so its panel samples the page as the rest of the chrome does. Focus stays inside,
 * Back or a tap outside closes it and gives focus back to the capsule; on a phone it follows the finger and a flick closes it.
 */
public final class HostSheet extends FrameLayout {
    /** A PC as the sheet says it. */
    static final class Row {
        final HomeModel.HostCard card;
        final String status;
        final int dot;
        final boolean current;

        Row(HomeModel.HostCard card, String status, int dot, boolean current) {
            this.card = card;
            this.status = status;
            this.dot = dot;
            this.current = current;
        }
    }

    /** What the row of this PC says. {@code game} is what Home knows it runs; {@code details} is null while the PC is not known yet. */
    static Row rowOf(Context context, HomeModel.HostCard card, ComputerDetails details, String game, boolean current) {
        PcGridAdapter.RowState state = details != null ? PcGridAdapter.rowState(details, false)
                : card.online ? PcGridAdapter.RowState.READY : PcGridAdapter.RowState.OFFLINE;
        String status;
        switch (state) {
            case RUNNING:
                status = game != null ? game : context.getString(R.string.pc_status_playing_unknown);
                break;
            case READY:
                status = context.getString(R.string.pc_status_online);
                break;
            case NOT_PAIRED:
                status = context.getString(R.string.pc_status_not_paired);
                break;
            case OFFLINE:
                status = context.getString(R.string.pc_offline);
                break;
            default:
                status = context.getString(R.string.pc_checking);
                break;
        }
        return new Row(card, status, PcGridAdapter.phoneDot(state), current);
    }

    private static final float DRAG_CLOSE_SHARE = .3f;
    private static final float FLICK_DP_PER_S = 900f;
    private static final float WIDE_SCALE_FROM = .96f;
    private static final long SCRIM_MS = 160L;

    private View scrim;
    private View panel;
    private LinearLayout rows;
    private View firstRow;
    private View anchor;
    private GlassForm form;
    private Runnable onClosed;
    private OnBackPressedCallback back;
    private SpringAnimation slide;
    private boolean open;
    private float downY;
    private boolean dragging;
    private VelocityTracker velocity;

    public HostSheet(Context context) {
        super(context);
    }

    public HostSheet(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    protected void onFinishInflate() {
        super.onFinishInflate();
        scrim = findViewById(R.id.host_sheet_scrim);
        panel = findViewById(R.id.host_sheet_panel);
        rows = findViewById(R.id.host_sheet_rows);
        scrim.setOnClickListener(v -> close());
    }

    /** Adds a closed sheet on top of {@code host}, which must be the activity's glass host. */
    static HostSheet attach(ComponentActivity activity, ViewGroup host, View anchor) {
        HostSheet sheet = (HostSheet) LayoutInflater.from(activity).inflate(R.layout.home_host_sheet, host, false);
        sheet.anchor = anchor;
        sheet.form = GlassForm.of(activity);
        sheet.back = new OnBackPressedCallback(false) {
            @Override public void handleOnBackPressed() {
                sheet.close();
            }
        };
        activity.getOnBackPressedDispatcher().addCallback(activity, sheet.back);
        host.addView(sheet);
        return sheet;
    }

    boolean isOpen() {
        return open;
    }

    /** Told when the sheet has gone, so nothing else opens over it. */
    void setOnClosed(Runnable onClosed) {
        this.onClosed = onClosed;
    }

    void show(List<Row> list, Consumer<HomeModel.HostCard> onPick) {
        rows.removeAllViews();
        View first = null;
        LayoutInflater inflater = LayoutInflater.from(getContext());
        for (Row row : list) {
            View view = inflater.inflate(R.layout.home_host_sheet_row, rows, false);
            ((ImageView) view.findViewById(R.id.host_row_dot)).setImageResource(row.dot);
            ((TextView) view.findViewById(R.id.host_row_name)).setText(row.card.name);
            ((TextView) view.findViewById(R.id.host_row_status)).setText(row.status);
            view.findViewById(R.id.host_row_mark).setVisibility(row.current ? VISIBLE : GONE);
            view.setSelected(row.current);
            view.setContentDescription(getContext().getString(R.string.library_item_description, row.card.name, row.status));
            view.setOnClickListener(v -> {
                close();
                onPick.accept(row.card);
            });
            rows.addView(view);
            if (first == null || row.current) first = view;
        }
        place();
        open = true;
        back.setEnabled(true);
        setVisibility(VISIBLE);
        hideBehind(true);
        if (first != null && !isInTouchMode()) {
            first.requestFocus();
        } else if (first != null) {
            // Under a finger no row shows a ring, but the page must not keep the keys: the next key starts on this row.
            firstRow = first;
            rows.setFocusable(true);
            rows.setFocusableInTouchMode(true);
            rows.requestFocus();
        }
        animateIn();
    }

    /** A phone's sheet stands on the foot of the window; elsewhere the panel hangs under the capsule, at its end. */
    private void place() {
        Resources res = getResources();
        float density = res.getDisplayMetrics().density;
        LayoutParams params = (LayoutParams) panel.getLayoutParams();
        int edge = Math.round(12 * density);
        boolean phone = form == GlassForm.PHONE;
        findViewById(R.id.host_sheet_grabber).setVisibility(phone ? VISIBLE : GONE);
        if (phone) {
            android.view.WindowInsets insets = getRootWindowInsets();
            int foot = insets == null ? 0 : insets.getSystemWindowInsetBottom();
            int radius = res.getDimensionPixelSize(R.dimen.nj_radius_sheet);
            params.width = LayoutParams.MATCH_PARENT;
            params.gravity = Gravity.BOTTOM;
            // Edge to edge, its lower corners past the foot of the window; its rows stay above them and the gesture bar.
            params.setMargins(0, 0, 0, -radius);
            View content = ((ViewGroup) panel).getChildAt(0);
            content.setPaddingRelative(content.getPaddingStart(), content.getPaddingTop(), content.getPaddingEnd(), edge + foot + radius);
        } else {
            int[] at = new int[2];
            int[] here = new int[2];
            anchor.getLocationInWindow(at);
            getLocationInWindow(here);
            params.width = Math.round(360 * density);
            params.gravity = Gravity.TOP | Gravity.LEFT;
            boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
            int start = rtl ? at[0] - here[0] : at[0] - here[0] + anchor.getWidth() - params.width;
            // Closed the sheet has no size yet: its parent's is what it will fill.
            int left = Math.max(edge, Math.min(start, ((View) getParent()).getWidth() - params.width - edge));
            params.setMargins(left, at[1] - here[1] + anchor.getHeight() + Math.round(8 * density), 0, edge);
        }
        panel.setLayoutParams(params);
    }

    private void animateIn() {
        float brightness = ResourcesCompat.getFloat(getResources(), isDay() ? R.dimen.nj_day_sheet_backdrop_brightness : R.dimen.nj_sheet_backdrop_brightness);
        float dim = 1f - brightness;
        scrim.animate().cancel();
        panel.animate().cancel();
        if (slide != null) slide.cancel();
        if (Motion.reduced(getContext())) {
            scrim.setAlpha(dim);
            panel.setTranslationY(0f);
            panel.setAlpha(1f);
            panel.setScaleX(1f);
            panel.setScaleY(1f);
            return;
        }
        scrim.setAlpha(0f);
        scrim.animate().alpha(dim).setDuration(SCRIM_MS).start();
        if (form == GlassForm.PHONE) {
            panel.setAlpha(1f);
            panel.setScaleX(1f);
            panel.setScaleY(1f);
            // Below the window on its first frame, whatever height it comes to.
            panel.setTranslationY(((View) getParent()).getHeight());
            panel.post(() -> slideTo(0f, null));
            return;
        }
        panel.setTranslationY(0f);
        panel.setPivotX(getLayoutDirection() == LAYOUT_DIRECTION_RTL ? 0f : panel.getLayoutParams().width);
        panel.setPivotY(0f);
        panel.setAlpha(0f);
        panel.setScaleX(WIDE_SCALE_FROM);
        panel.setScaleY(WIDE_SCALE_FROM);
        panel.animate().alpha(1f).scaleX(1f).scaleY(1f).setDuration(SCRIM_MS).start();
    }

    /** Springs the panel to this offset from wherever it is, on the sheet's token; the next input can retarget it. */
    private void slideTo(float y, Runnable then) {
        if (slide == null) {
            slide = new SpringAnimation(panel, DynamicAnimation.TRANSLATION_Y).setSpring(new SpringForce()
                    .setStiffness(ResourcesCompat.getFloat(getResources(), R.dimen.nj_motion_sheet_stiffness))
                    .setDampingRatio(ResourcesCompat.getFloat(getResources(), R.dimen.nj_motion_sheet_ratio)));
        }
        slide.cancel();
        slide.getSpring().setFinalPosition(y);
        if (then != null) {
            slide.addEndListener(new DynamicAnimation.OnAnimationEndListener() {
                @Override public void onAnimationEnd(DynamicAnimation animation, boolean canceled, float value, float speed) {
                    animation.removeEndListener(this);
                    if (!canceled) then.run();
                }
            });
        }
        slide.start();
    }

    void close() {
        if (!open) return;
        open = false;
        back.setEnabled(false);
        hideBehind(false);
        if (anchor != null && !isInTouchMode()) anchor.requestFocus();
        if (onClosed != null) onClosed.run();
        scrim.animate().cancel();
        panel.animate().cancel();
        if (Motion.reduced(getContext())) {
            gone();
            return;
        }
        scrim.animate().alpha(0f).setDuration(SCRIM_MS).start();
        if (form == GlassForm.PHONE) {
            slideTo(getHeight() - panel.getTop(), this::gone);
        } else {
            panel.animate().alpha(0f).scaleX(WIDE_SCALE_FROM).scaleY(WIDE_SCALE_FROM).setDuration(SCRIM_MS).withEndAction(this::gone).start();
        }
    }

    private void gone() {
        if (!open) setVisibility(GONE);
    }

    /** While it is open a screen reader stays in the sheet. */
    private void hideBehind(boolean hide) {
        ViewGroup host = (ViewGroup) getParent();
        if (host == null) return;
        for (int i = 0; i < host.getChildCount(); i++) {
            View child = host.getChildAt(i);
            if (child != this) {
                child.setImportantForAccessibility(hide ? IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS : IMPORTANT_FOR_ACCESSIBILITY_AUTO);
            }
        }
    }

    private boolean isDay() {
        return (getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK) != Configuration.UI_MODE_NIGHT_YES;
    }

    /** The keys walk the rows and stop at their ends. */
    @Override
    public View focusSearch(View focused, int direction) {
        if (!open) return super.focusSearch(focused, direction);
        if (focused == rows) {
            rows.setFocusable(false);
            rows.setFocusableInTouchMode(false);
            return firstRow;
        }
        View next = FocusFinder.getInstance().findNextFocus(this, focused, direction);
        return next != null ? next : focused;
    }

    @Override
    public boolean onInterceptTouchEvent(MotionEvent event) {
        if (form != GlassForm.PHONE || !open) return false;
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                downY = event.getRawY();
                dragging = false;
                return false;
            case MotionEvent.ACTION_MOVE:
                if (event.getRawY() - downY > ViewConfiguration.get(getContext()).getScaledTouchSlop() && overPanel(event)) {
                    dragging = true;
                    velocity = VelocityTracker.obtain();
                    if (slide != null) slide.cancel();
                    return true;
                }
                return false;
            default:
                return false;
        }
    }

    private boolean overPanel(MotionEvent event) {
        Rect box = new Rect();
        panel.getHitRect(box);
        return box.contains((int) event.getX(), (int) event.getY());
    }

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        if (!dragging) return super.onTouchEvent(event);
        velocity.addMovement(event);
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_MOVE:
                panel.setTranslationY(Math.max(0f, event.getRawY() - downY));
                return true;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_CANCEL:
                velocity.computeCurrentVelocity(1000);
                float speed = velocity.getYVelocity() / getResources().getDisplayMetrics().density;
                velocity.recycle();
                velocity = null;
                dragging = false;
                if (speed > FLICK_DP_PER_S || panel.getTranslationY() > panel.getHeight() * DRAG_CLOSE_SHARE) close();
                else slideTo(0f, null);
                return true;
            default:
                return true;
        }
    }
}

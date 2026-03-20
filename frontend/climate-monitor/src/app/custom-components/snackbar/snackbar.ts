import { Component, Inject, OnDestroy, OnInit } from '@angular/core';
import { MatSnackBarRef, MAT_SNACK_BAR_DATA } from '@angular/material/snack-bar';

@Component({
  selector: 'app-snackbar',
  templateUrl: './snackbar.html',
  standalone: false,
  styleUrls: ['./snackbar.css']
})
export class Snackbar implements OnInit, OnDestroy {
  actionType:string = ""
  private dismissTimeoutId?: ReturnType<typeof setTimeout>;
  private dismissStartedAt = 0;
  private remainingDurationMs = 0;

  constructor(public snackBarRef: MatSnackBarRef<Snackbar>, @Inject(MAT_SNACK_BAR_DATA) public data: any) { }

  ngOnInit(): void {
    const configuredDuration = Number(this.data?.durationMs ?? 3000);
    this.remainingDurationMs = Number.isFinite(configuredDuration) && configuredDuration > 0
      ? configuredDuration
      : 0;

    this.startDismissTimer();
  }

  ngOnDestroy(): void {
    this.clearDismissTimer();
  }

  onMouseEnter(): void {
    if (!this.dismissTimeoutId || this.remainingDurationMs <= 0) {
      return;
    }

    const elapsedMs = Date.now() - this.dismissStartedAt;
    this.remainingDurationMs = Math.max(0, this.remainingDurationMs - elapsedMs);
    this.clearDismissTimer();
  }

  onMouseLeave(): void {
    this.startDismissTimer();
  }

  goBack(){
    this.actionType = "back"
    this.snackBarRef.dismissWithAction()
  }

  onAddNew(link:string){
    this.actionType = "new"
    this.snackBarRef.dismissWithAction()
  }

  private startDismissTimer(): void {
    if (this.remainingDurationMs <= 0) {
      return;
    }

    this.clearDismissTimer();
    this.dismissStartedAt = Date.now();
    this.dismissTimeoutId = setTimeout(() => {
      this.dismissTimeoutId = undefined;
      this.snackBarRef.dismiss();
    }, this.remainingDurationMs);
  }

  private clearDismissTimer(): void {
    if (!this.dismissTimeoutId) {
      return;
    }

    clearTimeout(this.dismissTimeoutId);
    this.dismissTimeoutId = undefined;
  }

}

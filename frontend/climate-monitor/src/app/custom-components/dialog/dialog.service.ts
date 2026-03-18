import { Component,  Inject, Injectable } from '@angular/core';
import { MatDialog, MatDialogRef, MAT_DIALOG_DATA } from '@angular/material/dialog';

export type ImageDeleteChoice = 'hide' | 'delete' | null;




@Injectable({
  providedIn: 'root'
})
export class DialogService {

  constructor(public dialog:MatDialog) {
  }

  openPrompt(title:string, message:string, object:string|number, message2?:string|null, messages?:{ bold: string; normal: string; }[]|null){
    const dialogRef = this.dialog.open(PromptDialog, {
      minWidth: '450px',
      restoreFocus: false,
      data: {title: title, message: message, object: object,  ds: this.dialog, message2:message2, messages:messages}
    })

    return dialogRef
  }

}

export interface promptData {
  title: string,
  message: string,
  object: string
  ds:any,
  message2:string,
  messages:{ bold: string; normal: string; }[],
  ls?:any
}



@Component({
  selector: 'prompt',
  templateUrl: 'dialog-prompt.html',
  standalone: false,
})
export class PromptDialog {

  constructor(
    public dialogRef: MatDialogRef<PromptDialog>,
    @Inject(MAT_DIALOG_DATA) public data: promptData) {}

  onNoClick(): void {
    this.dialogRef.close();
  }

}


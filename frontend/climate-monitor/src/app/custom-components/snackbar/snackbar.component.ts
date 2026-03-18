import { Component, Inject, OnInit } from '@angular/core';
import { MatSnackBarRef, MAT_SNACK_BAR_DATA } from '@angular/material/snack-bar';

@Component({
  selector: 'app-snackbar',
  templateUrl: './snackbar.component.html',
  standalone: false,
  styleUrls: ['./snackbar.component.css']
})
export class SnackbarComponent implements OnInit {
  actionType:string = ""

  constructor(public snackBarRef: MatSnackBarRef<SnackbarComponent>, @Inject(MAT_SNACK_BAR_DATA) public data: any) { }

  ngOnInit(): void {
  }

  goBack(){
    console.log("Go back pressed")
    this.actionType = "back"
    this.snackBarRef.dismissWithAction()
  }

  onAddNew(link:string){
    console.log("Add new pressed", link)
    this.actionType = "new"
    this.snackBarRef.dismissWithAction()
    // this.snackBarRef.dismissWithAction()
  }

}

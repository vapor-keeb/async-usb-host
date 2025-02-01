use core::future::Future;

pub fn toss_potato<'a, T: 'static + ?Sized, R>(
    potato: &'a mut T,
    use_fn: impl FnOnce(&'static mut T) -> (&'static mut T, R),
) -> R {
    let static_potato: &'static mut T = unsafe { core::mem::transmute(potato) };
    let potato_ptr = static_potato as *mut T;

    let returned_potato = use_fn(static_potato);
    assert!(returned_potato.0 as *mut T == potato_ptr);
    returned_potato.1
}

pub async fn toss_potato_async<'a, T: 'static + ?Sized, R, Fut: Future<Output = (&'static mut T, R)>>(
    potato: &'a mut T,
    use_fn: impl FnOnce(&'static mut T) -> Fut,
) -> R {
    let static_potato: &'static mut T = unsafe { core::mem::transmute(potato) };
    let potato_ptr = static_potato as *mut T;

    let returned_potato = use_fn(static_potato).await;
    assert!(returned_potato.0 as *mut T == potato_ptr);
    returned_potato.1
}
